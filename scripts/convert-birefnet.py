#!/usr/bin/env python3
# /// script
# requires-python = ">=3.12,<3.13"
# dependencies = [
#   "torch==2.14.1",
#   "torchvision==0.29.1",
#   "transformers==5.19.0",
#   "timm==1.0.30",
#   "kornia==0.8.3",
#   "einops==0.8.2",
#   "safetensors==0.8.0",
#   "onnx==1.23.2",
#   "onnxscript==0.7.2",
#   "onnxruntime==1.28.0",
#   "numpy==2.5.3",
#   "pillow==12.3.0",
# ]
# [tool.uv]
# exclude-newer = "2026-10-09T00:00:00Z"
# [[tool.uv.index]]
# name = "pytorch-cpu"
# url = "https://download.pytorch.org/whl/cpu"
# explicit = true
# [tool.uv.sources]
# torch = { index = "pytorch-cpu" }
# torchvision = { index = "pytorch-cpu" }
# ///
"""Export BiRefNet_lite (MIT) to ONNX with native DeformConv nodes and prove it matches PyTorch.

The onnx-community export Nuzky used before decomposes the 20 deformable convolutions of the
decoder into gathers over [1, 64, 49, 256, 256] tensors: 5.5-9 s and 6-11 GB per frame on the CPU.
Here torch.onnx (dynamo) maps torchvision::deform_conv2d to the standard ONNX DeformConv op
through onnxscript's torchlib, and the model targets opset 22, the first opset ONNX Runtime 1.28
has a CPU DeformConv kernel for. The contract stays the same: input `input_image` float32
[1,3,1024,1024] (RGB, ImageNet mean/std, NCHW), output `output_image` float32 [1,1,1024,1024]
logits, fp32 weights.

The script downloads the version-pinned model code and weights from Hugging Face and checks their
SHA-256, loads them the way the model card does (transformers, trust_remote_code, here from the
verified local copy), exports, then compares PyTorch fp32, the new ONNX and the onnx-community ONNX
on ONNX Runtime 1.28.0 (the newest 1.28 wheel on PyPI) on real images and noise. It fails when the
new file differs from PyTorch by more than 1e-3 in probability.

The weights were uploaded on 2024-08-02 with code that stacked the image patches of the decoder's
input blocks column by column (get_patches_batch). Since 2024-11 the repo's image2patches stacks them
in another channel order, so the pinned code feeds four of those blocks permuted channels. Nuzky ships
the order the weights were trained with, which the onnx-community file also reproduces, so masks stay
as they were: out/birefnet-lite.onnx. `--patch-order pinned` follows the pinned code instead and
writes out/birefnet-lite-pinned-order.onnx.

`--bench` also measures session load time, run time and peak RSS of both files with the ONNX Runtime
1.28.3 library the app ships, through a small C runner built with gcc, default session options.

Run from the repository root (everything, including the uv cache, stays in the gitignored
tmp-test/birefnet-export/):

    UV_CACHE_DIR=tmp-test/birefnet-export/uv-cache uv run --python 3.12 --script scripts/convert-birefnet.py \
        [--patch-order training|pinned] [--bench]

Needs `ffmpeg` on PATH, `tmp-test/face-open.png` and `face-blink.png` from scripts/fixtures.sh,
and gcc for --bench. Output: tmp-test/birefnet-export/out/birefnet-lite.onnx and report.json.
"""

import argparse
import hashlib
import json
import os
import shutil
import statistics
import subprocess
import sys
import tarfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "tmp-test" / "birefnet-export"
DOWNLOADS = WORK / "downloads"
MODEL_DIR = DOWNLOADS / "BiRefNet_lite"
OUT = WORK / "out"
OUTPUT = OUT / "birefnet-lite.onnx"

os.environ["HF_HOME"] = str(WORK / "hf")  # transformers copies the remote code module here
os.environ["HF_HUB_OFFLINE"] = "1"
os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"

import numpy as np  # noqa: E402

OPSET = 22
SIDE = 1024
MEAN = np.array([0.485, 0.456, 0.406], np.float32)
STD = np.array([0.229, 0.224, 0.225], np.float32)
MAX_PROB_DIFF = 1e-3

REPO_COMMIT = "aa62cd87eafb9cc43056d08ef3615a14628b831d"
HF = f"https://huggingface.co/ZhengPeng7/BiRefNet_lite/resolve/{REPO_COMMIT}/"
LICENSE_URL = "https://raw.githubusercontent.com/ZhengPeng7/BiRefNet/ebcc0bc8ec7fe919cec829f2dea656b3078acddc/LICENSE"
COMMUNITY_URL = (
    "https://huggingface.co/onnx-community/BiRefNet_lite-ONNX/resolve/"
    "de15b22ba131738a16dff04aab8bdf8dc32e3ac1/onnx/model.onnx"
)
ORT_VERSION = "1.28.3"
COMMONS = "https://upload.wikimedia.org/wikipedia/commons/"

# name -> (url, sha256)
SOURCES = {
    "BiRefNet_lite/config.json": (HF + "config.json", "9dc8614fccddd40c601aeadc69b9db6dd820598179b2a2198492e6ffa016a824"),
    "BiRefNet_lite/BiRefNet_config.py": (HF + "BiRefNet_config.py", "e7b8c2a74f6cea6a59553d517f71d47f2c1d90e670a13416af17c25fe2f3dc52"),
    "BiRefNet_lite/birefnet.py": (HF + "birefnet.py", "af8568b5be406bf4d2a68a7ed6d72e40f73b37a1fb6fc9ebd71b5b3cbcd069c9"),
    "BiRefNet_lite/model.safetensors": (HF + "model.safetensors", "4417d89795250e698c3cb0ae8df15743810065f646f48a694fdfa7ca052d0815"),
    # The model card, `license: mit` in its front matter; the Hugging Face repo has no LICENSE file.
    "BiRefNet_lite/README.md": (HF + "README.md", "22fe432bdd706e8079ee1c53dcd6192962291d023c84d5416df3c983f6e665a2"),
    # The MIT licence of the upstream GitHub repo the model card links to.
    "LICENSE": (LICENSE_URL, "92a7089e0915fc32bc40067560b398f1e6a7a5958abd7d04eda393629a5acefb"),
    "onnx-community-model.onnx": (COMMUNITY_URL, "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333"),
    # Parity inputs, test data only, never shipped. CC0 photos from Wikimedia Commons:
    # commons.wikimedia.org/wiki/File:Tabby_cat_with_blue_eyes-3336579.jpg, ...Red_Smart_Car_Side_View_Driveway.jpg,
    # ...Coffee_Cup_on_Modern_Table.jpg, ...Bike_Rack_at_Bandar_Tropicana_Aman.jpg,
    # ...Landscape-wilderness-mountain-meadow-hill-lake-694751.jpg
    "parity/cat.jpg": (COMMONS + "c/c7/Tabby_cat_with_blue_eyes-3336579.jpg", "f91f1e37a23344251f40a7731b28b6612fef6cc37a2f1e7f97d5f6610063248c"),
    "parity/car.jpg": (COMMONS + "f/f8/Red_Smart_Car_Side_View_Driveway.jpg", "17718e32e4b30660d8c480ef39895ad293a9cb647ea79b3567f485b2f60d18ce"),
    "parity/coffee.jpg": (COMMONS + "a/a8/Coffee_Cup_on_Modern_Table.jpg", "c35248ded83965ae4c0ac4b76012c78acc529d94766123cbdea25dec6c3399ed"),
    "parity/bike-rack.jpg": (COMMONS + "4/42/Bike_Rack_at_Bandar_Tropicana_Aman.jpg", "3779085c82dea1f1c9474e6835b26287f1a6f48824d6ffc4e7e48b92c85cb035"),
    "parity/landscape.jpg": (COMMONS + "1/19/Landscape-wilderness-mountain-meadow-hill-lake-694751.jpg", "46a2347647fea2442544dfdad1cbc763b7924a48116c2754ed1877d40d5003e8"),
    # Public domain (NOAA/NASA), commons.wikimedia.org/wiki/File:NOAA_Interview_Opportunity-_Ready_to_GOES!_NOAA%E2%80%99s_Latest_GOES_Weather_Satellite_Ready_To_Launch_Next_Week!_(SVS14602_-_GOES-U_Canned_Side_Interview_Pam_Sullivan).webm
    "parity/pam_sullivan.webm": (
        COMMONS + "4/42/NOAA_Interview_Opportunity-_Ready_to_GOES%21_NOAA%E2%80%99s_Latest_GOES_Weather_Satellite_Ready_To_Launch_Next_Week%21_%28SVS14602_-_GOES-U_Canned_Side_Interview_Pam_Sullivan%29.webm",
        "8074c56d64204ad9b44c0cde54ab6ea0ac04792f8a79f1b2b9bc8eccedf6facf",
    ),
    # The ONNX Runtime the app ships (scripts/fetch-onnxruntime.mjs), only for --bench.
    f"onnxruntime-linux-x64-{ORT_VERSION}.tgz": (
        f"https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/onnxruntime-linux-x64-{ORT_VERSION}.tgz",
        "db14e4863bd37893fc59729d986ab2a0d043d10b7d44da1913c4982b7e3d009c",
    ),
}
# Seconds into the video: a white text card on black, the interviewee, the interviewee gesturing.
VIDEO_TIMES = [15.0, 30.0, 195.0]


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def fetch(name):
    url, digest = SOURCES[name]
    path = DOWNLOADS / name
    if path.exists() and sha256(path) == digest:
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    part = path.with_name(path.name + ".part")
    req = urllib.request.Request(url, headers={"User-Agent": "nuzky-convert-birefnet/1 (https://nuzky.app)"})
    with urllib.request.urlopen(req, timeout=120) as r, open(part, "wb") as f:
        shutil.copyfileobj(r, f, 1 << 20)
    got = sha256(part)
    if got != digest:
        part.unlink()
        sys.exit(f"SHA-256 mismatch for {name}: expected {digest}, got {got}")
    part.rename(path)
    return path


def load_model():
    """The model card's way: AutoModelForImageSegmentation with the repo's own code."""
    import torch
    from safetensors.torch import load_file
    from transformers import AutoModelForImageSegmentation

    model, info = AutoModelForImageSegmentation.from_pretrained(
        str(MODEL_DIR), trust_remote_code=True, local_files_only=True, dtype=torch.float32, output_loading_info=True
    )
    if any(info[k] for k in ("missing_keys", "unexpected_keys", "mismatched_keys", "error_msgs")):
        sys.exit(f"Weights did not load cleanly: {info}")
    model.eval()
    # Every tensor of the checkpoint sits in the model unchanged, and nothing else does.
    checkpoint = load_file(MODEL_DIR / "model.safetensors")
    state = model.state_dict()
    if set(checkpoint) != set(state) or any(not torch.equal(checkpoint[k], state[k]) for k in checkpoint):
        sys.exit("The loaded model differs from model.safetensors")
    return model


def use_training_patch_order(model):
    """Stack the image patches in the order of get_patches_batch from the code the weights shipped with
    (huggingface.co/ZhengPeng7/BiRefNet_lite/blob/224e9022586d/birefnet.py): columns outer, rows inner,
    channels innermost. The pinned image2patches uses 'b (c hg wg) h w' instead."""
    from einops import rearrange

    def image2patches(image, grid_h=2, grid_w=2, patch_ref=None, transformation=None):
        grid_h, grid_w = image.shape[-2] // patch_ref.shape[-2], image.shape[-1] // patch_ref.shape[-1]
        return rearrange(image, "b c (hg h) (wg w) -> b (wg hg c) h w", hg=grid_h, wg=grid_w)

    sys.modules[type(model).__module__].image2patches = image2patches


def export(model, patch_order):
    import onnx
    import torch

    class Logits(torch.nn.Module):
        """In eval mode BiRefNet returns a list whose last entry is the full-size logit map."""

        def __init__(self, inner):
            super().__init__()
            self.inner = inner

        def forward(self, x):
            return self.inner(x)[-1]

    example = torch.zeros(1, 3, SIDE, SIDE)
    program = torch.onnx.export(
        Logits(model).eval(),
        (example,),
        dynamo=True,
        opset_version=OPSET,
        input_names=["input_image"],
        output_names=["output_image"],
        external_data=False,
    )
    proto = program.model_proto
    props = {
        "title": "BiRefNet_lite subject mask",
        "license": "MIT, Copyright (c) 2024 ZhengPeng",
        "license_url": LICENSE_URL,
        "source": f"https://huggingface.co/ZhengPeng7/BiRefNet_lite/tree/{REPO_COMMIT}",
        "source_sha256": SOURCES["BiRefNet_lite/model.safetensors"][1],
        "converter": "scripts/convert-birefnet.py (torch 2.14.1 dynamo export, DeformConv at opset 22)",
        "patch_order": {"pinned": "image2patches of the pinned code", "training": "get_patches_batch of 2024-08-02"}[patch_order],
        "io": "input_image f32[1,3,1024,1024] RGB ImageNet-normalised NCHW -> output_image f32[1,1,1024,1024] logits",
    }
    del proto.metadata_props[:]
    for key, value in props.items():
        proto.metadata_props.add(key=key, value=value)
    onnx.checker.check_model(proto, full_check=True)
    check_graph(proto)
    OUT.mkdir(parents=True, exist_ok=True)
    part = OUTPUT.with_name(OUTPUT.name + ".part")
    onnx.save_model(proto, part)
    part.rename(OUTPUT)
    return proto


def check_graph(proto):
    import onnx

    def io(values):
        return [(v.name, v.type.tensor_type.elem_type, [d.dim_value for d in v.type.tensor_type.shape.dim]) for v in values]

    float32 = onnx.TensorProto.FLOAT
    if io(proto.graph.input) != [("input_image", float32, [1, 3, SIDE, SIDE])]:
        sys.exit(f"Unexpected inputs {io(proto.graph.input)}")
    if io(proto.graph.output) != [("output_image", float32, [1, 1, SIDE, SIDE])]:
        sys.exit(f"Unexpected outputs {io(proto.graph.output)}")
    if [(o.domain, o.version) for o in proto.opset_import] != [("", OPSET)] or len(proto.functions):
        sys.exit(f"Unexpected opsets {proto.opset_import} or local functions")
    ops = [n.op_type for n in proto.graph.node]
    if ops.count("DeformConv") != 20 or {"GatherND", "GatherElements"} & set(ops):
        sys.exit(f"Expected 20 DeformConv nodes and no gather-based sampling, got {ops.count('DeformConv')}")
    weights = {onnx.TensorProto.DataType.Name(t.data_type) for t in proto.graph.initializer}
    if not weights <= {"FLOAT", "INT64"}:
        sys.exit(f"Unexpected initializer types {weights}")


def normalise(image):
    """Squeeze the whole picture to 1024x1024 like crates/vision does, then ImageNet mean/std."""
    from PIL import Image

    rgb = np.asarray(image.convert("RGB").resize((SIDE, SIDE), Image.BILINEAR), np.float32) / 255.0
    return ((rgb - MEAN) / STD).transpose(2, 0, 1)[None].copy()


def parity_inputs():
    from PIL import Image, ImageOps

    inputs = {}
    for name in ("face-open", "face-blink"):
        path = ROOT / "tmp-test" / f"{name}.png"
        if not path.exists():
            sys.exit(f"{path} is missing; run scripts/fixtures.sh first")
        inputs[name] = normalise(Image.open(path))
    for name in ("cat", "car", "coffee", "bike-rack", "landscape"):
        inputs[name] = normalise(ImageOps.exif_transpose(Image.open(fetch(f"parity/{name}.jpg"))))
    video = fetch("parity/pam_sullivan.webm")
    for t in VIDEO_TIMES:
        raw = subprocess.run(
            ["ffmpeg", "-v", "error", "-ss", f"{t:.3f}", "-i", str(video), "-frames:v", "1", "-f", "rawvideo",
             "-pix_fmt", "rgb24", "-"],
            check=True, capture_output=True,
        ).stdout
        inputs[f"video-{t:g}s"] = normalise(Image.frombytes("RGB", (1920, 1080), raw))
    rng = np.random.default_rng(20261009)
    inputs["noise-normal"] = rng.standard_normal((1, 3, SIDE, SIDE), dtype=np.float32)
    pixels = rng.integers(0, 256, (SIDE, SIDE, 3), dtype=np.uint8)
    inputs["noise-pixels"] = normalise(Image.fromarray(pixels))
    return inputs


def sigmoid(logits):
    with np.errstate(over="ignore"):
        return 1.0 / (1.0 + np.exp(-logits.astype(np.float64)))


def compare(a, b):
    pa, pb = sigmoid(a), sigmoid(b)
    diff = np.abs(pa - pb)
    ma, mb = pa > 0.5, pb > 0.5
    union = int((ma | mb).sum())
    return {
        "max": float(diff.max()),
        "mean": float(diff.mean()),
        "iou": float((ma & mb).sum() / union) if union else 1.0,
        "flipped_pixels": int((ma ^ mb).sum()),
    }


def run_parity(model, inputs, masks):
    import onnxruntime as ort
    import torch
    from PIL import Image

    logits = {"torch": {}, "ours": {}, "community": {}}
    with torch.inference_mode():
        for name, x in inputs.items():
            logits["torch"][name] = model(torch.from_numpy(x))[-1].numpy()
    ours = ort.InferenceSession(str(OUTPUT), providers=["CPUExecutionProvider"])
    for name, x in inputs.items():
        logits["ours"][name] = ours.run(None, {"input_image": x})[0]
    del ours
    lean = ort.SessionOptions()  # same numbers, less memory for the gather-heavy graph
    lean.enable_cpu_mem_arena = False
    lean.enable_mem_pattern = False
    community = ort.InferenceSession(str(fetch("onnx-community-model.onnx")), lean, providers=["CPUExecutionProvider"])
    for name, x in inputs.items():
        logits["community"][name] = community.run(None, {"input_image": x})[0]
    del community

    masks.mkdir(parents=True, exist_ok=True)
    rows = {}
    for name in inputs:
        t, o, c = logits["torch"][name], logits["ours"][name], logits["community"][name]
        rows[name] = {
            "foreground_share": float((sigmoid(t) > 0.5).mean()),
            "ours_vs_torch": compare(o, t),
            "community_vs_torch": compare(c, t),
            "ours_vs_community": compare(o, c),
        }
        Image.fromarray((sigmoid(o)[0, 0] * 255).round().astype(np.uint8)).save(masks / f"{name}.png")
    return rows, logits


RUNNER_C = r"""
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "onnxruntime_c_api.h"

static const OrtApi *api;
static void check(OrtStatus *s) {
  if (s) { fprintf(stderr, "ONNX Runtime: %s\n", api->GetErrorMessage(s)); exit(1); }
}
static double now_ms(void) {
  struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec * 1e3 + t.tv_nsec / 1e6;
}
static long hwm_kb(void) {
  FILE *f = fopen("/proc/self/status", "r"); char line[256]; long kb = -1;
  while (fgets(line, sizeof line, f)) if (!strncmp(line, "VmHWM:", 6)) kb = atol(line + 6);
  fclose(f); return kb;
}

/* runner MODEL INPUT.f32 OUTPUT.f32|- RUNS default|lean: prints one JSON line. */
int main(int argc, char **argv) {
  if (argc != 6) { fprintf(stderr, "usage: runner MODEL INPUT OUTPUT|- RUNS default|lean\n"); return 2; }
  const size_t n = 3 * 1024 * 1024, m = 1024 * 1024;
  float *input = malloc(n * sizeof(float));
  FILE *f = fopen(argv[2], "rb");
  if (!f || fread(input, sizeof(float), n, f) != n) { fprintf(stderr, "cannot read %s\n", argv[2]); return 1; }
  fclose(f);
  api = OrtGetApiBase()->GetApi(ORT_API_VERSION);
  if (!api) { fprintf(stderr, "ONNX Runtime API %d unavailable\n", ORT_API_VERSION); return 1; }
  OrtEnv *env; check(api->CreateEnv(ORT_LOGGING_LEVEL_WARNING, "bench", &env));
  OrtSessionOptions *options; check(api->CreateSessionOptions(&options));
  if (!strcmp(argv[5], "lean")) { check(api->DisableCpuMemArena(options)); check(api->DisableMemPattern(options)); }
  long start = hwm_kb();
  double t0 = now_ms();
  OrtSession *session; check(api->CreateSession(env, argv[1], options, &session));
  printf("{\"version\": \"%s\", \"load_ms\": %.1f, \"hwm_start_kb\": %ld, \"hwm_load_kb\": %ld, \"runs\": [",
         OrtGetApiBase()->GetVersionString(), now_ms() - t0, start, hwm_kb());
  OrtMemoryInfo *info; check(api->CreateCpuMemoryInfo(OrtArenaAllocator, OrtMemTypeDefault, &info));
  int64_t shape[4] = {1, 3, 1024, 1024};
  OrtValue *x = NULL;
  check(api->CreateTensorWithDataAsOrtValue(info, input, n * sizeof(float), shape, 4,
                                            ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT, &x));
  const char *in_names[] = {"input_image"}, *out_names[] = {"output_image"};
  for (int i = 0; i < atoi(argv[4]); i++) {
    OrtValue *y = NULL;
    t0 = now_ms();
    check(api->Run(session, NULL, in_names, (const OrtValue *const *)&x, 1, out_names, 1, &y));
    double ms = now_ms() - t0;
    OrtTensorTypeAndShapeInfo *shape_info; size_t count;
    check(api->GetTensorTypeAndShape(y, &shape_info)); check(api->GetTensorShapeElementCount(shape_info, &count));
    api->ReleaseTensorTypeAndShapeInfo(shape_info);
    if (count != m) { fprintf(stderr, "unexpected output size %zu\n", count); return 1; }
    if (i == 0 && strcmp(argv[3], "-")) {
      float *out; check(api->GetTensorMutableData(y, (void **)&out));
      FILE *o = fopen(argv[3], "wb"); fwrite(out, sizeof(float), m, o); fclose(o);
    }
    api->ReleaseValue(y);
    printf("%s{\"ms\": %.1f, \"hwm_kb\": %ld}", i ? ", " : "", ms, hwm_kb());
    fflush(stdout);
  }
  printf("]}\n");
  api->ReleaseValue(x); api->ReleaseMemoryInfo(info); api->ReleaseSession(session);
  api->ReleaseSessionOptions(options); api->ReleaseEnv(env); free(input);
  return 0;
}
"""


def build_runner():
    bench = WORK / "bench"
    ort_dir = bench / f"onnxruntime-{ORT_VERSION}"
    lib = ort_dir / "lib" / f"libonnxruntime.so.{ORT_VERSION}"
    if not lib.exists():
        prefix = f"onnxruntime-linux-x64-{ORT_VERSION}/"
        with tarfile.open(fetch(f"onnxruntime-linux-x64-{ORT_VERSION}.tgz")) as tar:
            members = [
                m for m in tar.getmembers()
                if m.name.startswith(prefix + "include/") or m.name in (prefix + "lib/" + lib.name, prefix + "lib/libonnxruntime.so.1")
            ]
            for m in members:
                m.name = m.name[len(prefix):]
            tar.extractall(ort_dir, members=members, filter="data")
    source = bench / "runner.c"
    source.write_text(RUNNER_C)
    runner = bench / "runner"
    subprocess.run(
        ["gcc", "-O2", "-std=c11", "-D_POSIX_C_SOURCE=199309L", str(source), "-I", str(ort_dir / "include"),
         "-L", str(lib.parent), f"-l:{lib.name}", f"-Wl,-rpath,{lib.parent}", "-o", str(runner)],
        check=True,
    )
    return runner


def run_bench(inputs, logits, reps):
    """Fresh process per measurement: load, one run (what the app does per mask), peak RSS."""
    runner = build_runner()
    bench = WORK / "bench"
    x = bench / "face-open.f32"
    inputs["face-open"].astype(np.float32).tofile(x)
    models = {"ours": OUTPUT, "community": fetch("onnx-community-model.onnx")}

    def measure(model, mode, runs, out="-"):
        rss = bench / "time.txt"
        proc = subprocess.run(
            ["/usr/bin/time", "-f", "%M", "-o", str(rss), str(runner), str(models[model]), str(x), out, str(runs), mode],
            check=True, capture_output=True, text=True,
        )
        result = json.loads(proc.stdout)
        result["max_rss_kb"] = int(rss.read_text().split()[-1])
        return result

    results = {"load_before": os.getloadavg(), "cpu": cpu_name(), "samples": {}}
    for model in models:  # also checks that 1.28.3 gives the numbers 1.28.0 gave
        out = bench / f"{model}.f32"
        first = measure(model, "default", 1, str(out))
        native = np.fromfile(out, np.float32).reshape(1, 1, SIDE, SIDE)
        results[f"{model}_1.28.3_vs_1.28.0"] = compare(native, logits[model]["face-open"])
        results["version"] = first["version"]
    for mode in ("default", "lean"):
        for _ in range(reps):
            for model in models:  # interleaved, so load from other work hits both alike
                results["samples"].setdefault(f"{model}/{mode}", []).append(measure(model, mode, 1))
    for model in models:
        results["samples"][f"{model}/default/3 runs"] = [measure(model, "default", 3)]
    results["load_after"] = os.getloadavg()
    summary = {}
    for key, samples in results["samples"].items():
        summary[key] = {
            "load_ms": statistics.median(s["load_ms"] for s in samples),
            "run_ms": [statistics.median(s["runs"][i]["ms"] for s in samples) for i in range(len(samples[0]["runs"]))],
            "peak_rss_mb": statistics.median(s["max_rss_kb"] for s in samples) / 1024,
            "rss_after_load_mb": statistics.median(s["hwm_load_kb"] for s in samples) / 1024,
            "n": len(samples),
        }
    results["summary"] = summary
    return results


def cpu_name():
    for line in Path("/proc/cpuinfo").read_text().splitlines():
        if line.startswith("model name"):
            return line.split(":", 1)[1].strip()
    return "unknown"


def main():
    global OUTPUT
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--patch-order", choices=("training", "pinned"), default="training",
                        help="channel order of the decoder's image patches (see the module docstring)")
    parser.add_argument("--bench", action="store_true", help="also measure cost with ONNX Runtime 1.28.3")
    parser.add_argument("--reps", type=int, default=5, help="processes per model and mode in --bench")
    args = parser.parse_args()
    suffix = "" if args.patch_order == "training" else "-pinned-order"
    OUTPUT = OUT / f"birefnet-lite{suffix}.onnx"

    for name in SOURCES:
        if name.startswith("BiRefNet_lite/") or name == "LICENSE":
            fetch(name)
    started = time.time()
    model = load_model()
    if args.patch_order == "training":
        use_training_patch_order(model)
    proto = export(model, args.patch_order)
    print(f"Exported {OUTPUT.relative_to(ROOT)} in {time.time() - started:.0f} s: {len(proto.graph.node)} nodes, "
          f"{[n.op_type for n in proto.graph.node].count('DeformConv')} DeformConv, opset {OPSET}, IR {proto.ir_version}")
    shutil.copyfile(DOWNLOADS / "LICENSE", OUT / "LICENSE-BiRefNet.txt")

    inputs = parity_inputs()
    rows, logits = run_parity(model, inputs, OUT / f"masks{suffix}")
    print(f"\n{'input':<14} {'fg':>5}  {'ours vs torch: max / mean / IoU':>34}  {'community vs torch':>30}  {'ours vs community':>30}")
    for name, row in rows.items():
        cells = [f"{r['max']:.2e} / {r['mean']:.1e} / {r['iou']:.5f}" for r in
                 (row["ours_vs_torch"], row["community_vs_torch"], row["ours_vs_community"])]
        print(f"{name:<14} {row['foreground_share']:5.3f}  {cells[0]:>34}  {cells[1]:>30}  {cells[2]:>30}")
    report = {
        "output": {"path": str(OUTPUT.relative_to(ROOT)), "bytes": OUTPUT.stat().st_size, "sha256": sha256(OUTPUT)},
        "sources": {name: {"url": url, "sha256": digest} for name, (url, digest) in SOURCES.items()},
        "graph": {"nodes": len(proto.graph.node), "opset": OPSET, "ir_version": proto.ir_version},
        "patch_order": args.patch_order,
        "parity": rows,
    }
    worst = max(row["ours_vs_torch"]["max"] for row in rows.values())
    if args.bench:
        report["bench"] = run_bench(inputs, logits, args.reps)
        print(f"\nONNX Runtime {report['bench']['version']} on {report['bench']['cpu']}, medians:")
        for key, s in report["bench"]["summary"].items():
            runs = ", ".join(f"{ms / 1000:.2f} s" for ms in s["run_ms"])
            print(f"  {key:<28} load {s['load_ms'] / 1000:.2f} s, run {runs}, peak RSS {s['peak_rss_mb'] / 1024:.2f} GB (n={s['n']})")
    (OUT / f"report{suffix}.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"\n{OUTPUT.relative_to(ROOT)}  {report['output']['bytes']} bytes  sha256 {report['output']['sha256']}")
    if worst > MAX_PROB_DIFF:
        sys.exit(f"FAIL: the export differs from PyTorch by {worst:.2e} in probability (limit {MAX_PROB_DIFF})")


if __name__ == "__main__":
    main()
