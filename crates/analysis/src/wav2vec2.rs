//! A wav2vec2 CTC acoustic model, as Hugging Face's `Wav2Vec2ForCTC` computes it, run with candle on
//! the CPU from a GGUF file. It turns 16 kHz mono sound into, for every 20 ms frame, how likely each
//! letter of the model's alphabet is; `align` reads the words' times from that.

use std::{fs::File, io::BufReader, path::Path};

use anyhow::{Context, Result, bail, ensure};
use candle_core::{D, DType, Device, Tensor, quantized::QTensor, quantized::gguf_file};

/// Samples per output frame and the samples one frame hears: the strides and kernels of the
/// feature encoder multiplied out.
pub(crate) const STRIDE: usize = 320;
pub(crate) const RECEPTIVE: usize = 400;

pub(crate) struct Wav2Vec2 {
    convs: Vec<Conv>,
    /// Layer norm after every convolution (large models), or a group norm after the first only.
    layer_norm_convs: bool,
    projection_norm: Norm,
    projection: Linear,
    position: Conv,
    position_groups: usize,
    encoder_norm: Norm,
    layers: Vec<Layer>,
    /// Large models normalise before attention and the feed-forward, base models after.
    stable: bool,
    heads: usize,
    head: Linear,
    /// The model's alphabet, by token id.
    pub tokens: Vec<String>,
    /// The CTC blank: the model's padding token.
    pub blank: usize,
    eps: f64,
}

struct Conv {
    weight: Tensor,
    bias: Option<Tensor>,
    stride: usize,
    norm: Option<Norm>,
}

struct Norm {
    weight: Tensor,
    bias: Tensor,
}

/// Weights stay quantised in memory and are expanded one layer at a time, so the model takes
/// little more memory than its file while the matrix products run at full float speed.
struct Linear {
    weight: QTensor,
    bias: Tensor,
}

struct Layer {
    q: Linear,
    k: Linear,
    v: Linear,
    out: Linear,
    norm1: Norm,
    fc1: Linear,
    fc2: Linear,
    norm2: Norm,
}

impl Wav2Vec2 {
    pub fn load(path: &Path) -> Result<Self> {
        let mut file = Gguf::open(path)?;
        ensure!(
            file.content.metadata.get("general.architecture").and_then(|v| v.to_string().ok()).map(String::as_str)
                == Some("wav2vec2"),
            "Word timing model is not a wav2vec2 model"
        );
        let hidden = file.number("hidden_size")?;
        let heads = file.number("num_attention_heads")?;
        ensure!(heads > 0 && hidden % heads == 0, "Word timing model has an invalid attention shape");
        let mut convs = Vec::new();
        for i in 0..file.number("num_feat_extract_layers")? {
            let has_bias = file.content.tensor_infos.contains_key(&format!("cnn.{i}.conv.bias"));
            convs.push(Conv {
                weight: file.dense(&format!("cnn.{i}.conv.weight"))?,
                bias: if has_bias { Some(file.dense(&format!("cnn.{i}.conv.bias"))?) } else { None },
                stride: file.number(&format!("conv_stride_{i}"))?,
                norm: if file.number(&format!("cnn_has_norm_{i}"))? == 1 {
                    Some(file.norm(&format!("cnn.{i}.norm"))?)
                } else {
                    None
                },
            });
        }
        let mut layers = Vec::new();
        for i in 0..file.number("num_hidden_layers")? {
            let p = format!("enc.{i}");
            layers.push(Layer {
                q: file.linear(&format!("{p}.attn.q"))?,
                k: file.linear(&format!("{p}.attn.k"))?,
                v: file.linear(&format!("{p}.attn.v"))?,
                out: file.linear(&format!("{p}.attn.out"))?,
                fc1: file.linear(&format!("{p}.ffn.fc1"))?,
                fc2: file.linear(&format!("{p}.ffn.fc2"))?,
                norm1: file.norm(&format!("{p}.ln1"))?,
                norm2: file.norm(&format!("{p}.ln2"))?,
            });
        }
        let tokens: Vec<String> = file
            .content
            .metadata
            .get("tokenizer.ggml.tokens")
            .context("Word timing model lacks its alphabet")?
            .to_vec()?
            .iter()
            .map(|v| v.to_string().cloned())
            .collect::<candle_core::Result<_>>()?;
        let blank = file
            .content
            .metadata
            .get("tokenizer.ggml.padding_token_id")
            .context("Word timing model lacks its blank")?
            .to_u32()? as usize;
        let model = Self {
            convs,
            layer_norm_convs: file.number("feat_extract_norm_type")? == 1,
            projection_norm: file.norm("feat_proj.ln")?,
            projection: file.linear("feat_proj")?,
            position: Conv {
                weight: file.dense("pos_conv.weight")?,
                bias: Some(file.dense("pos_conv.bias")?),
                stride: 1,
                norm: None,
            },
            position_groups: file.number("num_conv_pos_embedding_groups")?,
            encoder_norm: file.norm("enc.ln")?,
            layers,
            stable: file.number("do_stable_layer_norm")? == 1,
            heads,
            head: file.linear("lm_head")?,
            tokens,
            blank,
            eps: file.meta("layer_norm_eps")?.to_f32()? as f64,
        };
        ensure!(
            model.blank < model.tokens.len() && model.head.bias.dim(0)? == model.tokens.len(),
            "Word timing model's alphabet does not match its output"
        );
        Ok(model)
    }

    /// Frames of `samples` samples: what the strided convolutions leave.
    pub fn frames(&self, samples: usize) -> usize {
        self.convs.iter().fold(samples, |len, conv| {
            let kernel = conv.weight.dim(2).unwrap_or(1);
            if len < kernel { 0 } else { (len - kernel) / conv.stride + 1 }
        })
    }

    /// Log-probabilities of every token in every 20 ms frame of 16 kHz mono `audio`, frames × tokens.
    /// `cancelled` is checked between layers.
    pub fn log_probs(&self, audio: &[f32], cancelled: &dyn Fn() -> bool) -> Result<Vec<f32>> {
        let stop = || -> Result<()> {
            if cancelled() {
                bail!("CANCELLED: transcription cancelled");
            }
            Ok(())
        };
        ensure!(audio.len() >= RECEPTIVE, "Too little sound for the word timing model");
        // The feature extractor's normalisation: zero mean and unit variance over the whole input.
        let mean = audio.iter().map(|&s| s as f64).sum::<f64>() / audio.len() as f64;
        let var = audio.iter().map(|&s| (s as f64 - mean).powi(2)).sum::<f64>() / audio.len() as f64;
        let scale = 1.0 / (var + 1e-7).sqrt();
        let normalised: Vec<f32> = audio.iter().map(|&s| ((s as f64 - mean) * scale) as f32).collect();
        let mut x = Tensor::from_vec(normalised, (1, 1, audio.len()), &Device::Cpu)?;
        for (i, conv) in self.convs.iter().enumerate() {
            stop()?;
            x = conv.apply(&x, 1)?;
            if let Some(norm) = &conv.norm {
                x = if self.layer_norm_convs {
                    norm.apply(&x.transpose(1, 2)?, self.eps)?.transpose(1, 2)?
                } else {
                    debug_assert_eq!(i, 0);
                    // A group norm with one group per channel: each channel over time.
                    let mean = x.mean_keepdim(2)?;
                    let centred = x.broadcast_sub(&mean)?;
                    let var = centred.sqr()?.mean_keepdim(2)?;
                    let x = centred.broadcast_div(&(var + 1e-5)?.sqrt()?)?;
                    x.broadcast_mul(&norm.weight.reshape((1, (), 1))?)?.broadcast_add(&norm.bias.reshape((
                        1,
                        (),
                        1,
                    ))?)?
                };
            }
            x = gelu(&x)?;
        }
        // [1, channels, frames] -> [frames, channels]
        let x = x.squeeze(0)?.t()?.contiguous()?;
        let mut h = self.projection.apply(&self.projection_norm.apply(&x, self.eps)?)?;
        stop()?;
        let position = self.position.apply(&h.t()?.unsqueeze(0)?.contiguous()?, self.position_groups)?;
        // An even kernel pads one frame too many; the model drops the last.
        let frames = h.dim(0)?;
        let position = gelu(&position.narrow(2, 0, frames)?)?.squeeze(0)?.t()?;
        h = (h + position)?;
        if !self.stable {
            h = self.encoder_norm.apply(&h, self.eps)?;
        }
        for layer in &self.layers {
            stop()?;
            h = if self.stable {
                let a = (&h + self.attention(layer, &layer.norm1.apply(&h, self.eps)?)?)?;
                let f = layer.fc2.apply(&gelu(&layer.fc1.apply(&layer.norm2.apply(&a, self.eps)?)?)?)?;
                (a + f)?
            } else {
                let a = layer.norm1.apply(&(&h + self.attention(layer, &h)?)?, self.eps)?;
                let f = layer.fc2.apply(&gelu(&layer.fc1.apply(&a)?)?)?;
                layer.norm2.apply(&(a + f)?, self.eps)?
            };
        }
        if self.stable {
            h = self.encoder_norm.apply(&h, self.eps)?;
        }
        stop()?;
        let logits = self.head.apply(&h)?;
        let max = logits.max_keepdim(D::Minus1)?;
        let shifted = logits.broadcast_sub(&max)?;
        let log_sum = shifted.exp()?.sum_keepdim(D::Minus1)?.log()?;
        Ok(shifted.broadcast_sub(&log_sum)?.to_dtype(DType::F32)?.flatten_all()?.to_vec1()?)
    }

    fn attention(&self, layer: &Layer, x: &Tensor) -> Result<Tensor> {
        let (frames, hidden) = x.dims2()?;
        let width = hidden / self.heads;
        let split = |t: Tensor| -> Result<Tensor> {
            Ok(t.reshape((frames, self.heads, width))?.transpose(0, 1)?.contiguous()?)
        };
        let q = split((layer.q.apply(x)? * (width as f64).powf(-0.5))?)?;
        let k = split(layer.k.apply(x)?)?;
        let v = split(layer.v.apply(x)?)?;
        let scores = q.matmul(&k.t()?)?;
        let weights = candle_nn::ops::softmax_last_dim(&scores)?;
        let mixed = weights.matmul(&v)?.transpose(0, 1)?.reshape((frames, hidden))?;
        layer.out.apply(&mixed)
    }
}

struct Gguf {
    content: gguf_file::Content,
    reader: BufReader<File>,
}

impl Gguf {
    fn open(path: &Path) -> Result<Self> {
        let mut reader = BufReader::new(File::open(path).context("Opening word timing model")?);
        let content = gguf_file::Content::read(&mut reader).context("Reading word timing model")?;
        Ok(Self { content, reader })
    }

    fn meta(&self, key: &str) -> Result<&gguf_file::Value> {
        self.content.metadata.get(&format!("wav2vec2.{key}")).with_context(|| format!("Word timing model lacks {key}"))
    }

    fn number(&self, key: &str) -> Result<usize> {
        Ok(self.meta(key)?.to_u32()? as usize)
    }

    fn quantised(&mut self, name: &str) -> Result<QTensor> {
        self.content
            .tensor(&mut self.reader, name, &Device::Cpu)
            .with_context(|| format!("Reading {name} of the word timing model"))
    }

    fn dense(&mut self, name: &str) -> Result<Tensor> {
        Ok(self.quantised(name)?.dequantize(&Device::Cpu)?)
    }

    fn norm(&mut self, name: &str) -> Result<Norm> {
        Ok(Norm { weight: self.dense(&format!("{name}.weight"))?, bias: self.dense(&format!("{name}.bias"))? })
    }

    fn linear(&mut self, name: &str) -> Result<Linear> {
        Ok(Linear { weight: self.quantised(&format!("{name}.weight"))?, bias: self.dense(&format!("{name}.bias"))? })
    }
}

impl Conv {
    /// `x` is [1, channels, length]. Same-length padding only for the positional convolution.
    fn apply(&self, x: &Tensor, groups: usize) -> Result<Tensor> {
        let padding = if groups > 1 { self.weight.dim(2)? / 2 } else { 0 };
        let y = x.conv1d(&self.weight, padding, self.stride, 1, groups)?;
        Ok(match &self.bias {
            Some(bias) => y.broadcast_add(&bias.reshape((1, (), 1))?)?,
            None => y,
        })
    }
}

impl Norm {
    /// Layer norm over the last dimension.
    fn apply(&self, x: &Tensor, eps: f64) -> Result<Tensor> {
        Ok(candle_nn::ops::layer_norm(&x.contiguous()?, &self.weight, &self.bias, eps as f32)?)
    }
}

/// The exact GELU, x·Φ(x), on all cores: one call covers millions of values.
fn gelu(x: &Tensor) -> Result<Tensor> {
    use rayon::prelude::*;
    let shape = x.shape().clone();
    let mut values = x.flatten_all()?.to_vec1::<f32>()?;
    values.par_chunks_mut(1 << 14).for_each(|chunk| {
        for v in chunk {
            *v = 0.5 * *v * (1.0 + libm::erff(*v * std::f32::consts::FRAC_1_SQRT_2));
        }
    });
    Ok(Tensor::from_vec(values, shape, &Device::Cpu)?)
}

impl Linear {
    /// `x` is [frames, in].
    fn apply(&self, x: &Tensor) -> Result<Tensor> {
        let weight = self.weight.dequantize(&Device::Cpu)?;
        Ok(x.matmul(&weight.t()?)?.broadcast_add(&self.bias)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 16 kHz mono sound of a fixture as alignment hears it.
    fn heard(root: &Path, file: &str) -> Vec<f32> {
        let asset = nuzky_engine::media::probe(&root.join(file), "oracle".into()).unwrap();
        let cache = root.join("tmp-test/analysis").join(format!("oracle-{}", std::process::id()));
        let pcm = crate::audio::open_pcm(&asset, &cache, &|| false).unwrap();
        pcm.samples().chunks(6).map(|s| s.iter().sum::<f32>() / s.len() as f32).collect()
    }

    /// Our model against what the official one computes on the same sound: the same most likely
    /// token in nearly every frame, and nearly its log-probability. The 8-bit Czech weights move
    /// a frame whose two best tokens were almost tied; the 16-bit English ones match closely.
    #[test]
    #[ignore = "needs tmp-test/krysar-cs.wav and the word timing models from scripts/fixtures.sh"]
    fn hears_what_the_official_models_hear() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let audio = heard(&root, "tmp-test/krysar-cs.wav");
        let models = root.join("tmp-test/xdg/data/nuzky/models");
        for (model, oracle, mean) in [
            ("wav2vec2-xls-r-300m-cs-250-q8_0.gguf", "krysar-cs-oracle.tsv", 0.01),
            ("wav2vec2-base-960h.gguf", "krysar-cs-en-oracle.tsv", 0.001),
        ] {
            let model = Wav2Vec2::load(&models.join(model)).unwrap();
            let ours = model.log_probs(&audio, &|| false).unwrap();
            let text = std::fs::read_to_string(root.join("crates/analysis/tests/data").join(oracle)).unwrap();
            let expected: Vec<(usize, f32)> = text
                .lines()
                .filter(|l| !l.starts_with('#'))
                .map(|l| {
                    let (token, p) = l.split_once('\t').unwrap();
                    (token.parse().unwrap(), p.parse().unwrap())
                })
                .collect();
            let tokens = model.tokens.len();
            assert_eq!(ours.len(), expected.len() * tokens, "{oracle}: frames");
            let argmax = |row: &[f32]| row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
            let rows: Vec<&[f32]> = ours.chunks(tokens).collect();
            let same = rows.iter().zip(&expected).filter(|(row, (token, _))| argmax(row) == *token).count();
            let drift = rows.iter().zip(&expected).map(|(row, &(t, p))| (row[t] - p).abs() as f64).sum::<f64>()
                / rows.len() as f64;
            assert!(
                same * 100 >= expected.len() * 99,
                "{oracle}: same top token in {same} of {} frames",
                expected.len()
            );
            assert!(drift <= mean, "{oracle}: mean log-probability drift {drift}");
        }
    }
}
