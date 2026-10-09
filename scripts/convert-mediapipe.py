#!/usr/bin/env python3
# /// script
# requires-python = ">=3.12,<3.13"
# dependencies = [
#   "tensorflow-cpu==2.21.0",
#   "tf2onnx==1.17.0",
#   "onnx==1.23.2",
#   "onnxruntime==1.28.0",
#   "ai-edge-litert==2.3.0",
#   "mediapipe==1.1.0",
#   "numpy==2.5.3",
#   "opencv-contrib-python==5.0.0.93",
#   "protobuf==7.36.2",
#   "flatbuffers==25.12.19",
# ]
# [tool.uv]
# exclude-newer = "2026-10-09T00:00:00Z"
# ///
"""Convert three Apache-2.0 MediaPipe TFLite models to ONNX and prove they match.

Models: Face Mesh V2 and Blendshape V2 (both inside face_landmarker.task) and the
square 256x256 Selfie Segmenter. The script downloads the version-pinned sources,
checks their SHA-256, converts them with tf2onnx (opset 17, fp32 weights), runs the
original TFLite and ONNX Runtime 1.28 (the version `ort` 2.0.0-rc.13 bundles) on the
same inputs and fails when they differ by more than the thresholds below. The face
models run in LiteRT; the segmenter's reference is MediaPipe's ImageSegmenter, because
LiteRT cannot load MediaPipe's custom Convolution2DTransposeBias op.

Parity inputs are real face crops from two public-domain NASA/NOAA interview videos
on Wikimedia Commons, cropped exactly like MediaPipe's tracking path, plus perturbed
and synthetic inputs.

Run from the repository root (everything, including the uv cache, stays in
tmp-test/vision-convert/, which is gitignored):

    UV_CACHE_DIR=tmp-test/vision-convert/uv-cache uv run --script scripts/convert-mediapipe.py

Needs `ffmpeg` and `ffprobe` on PATH. Outputs land in tmp-test/vision-convert/out/.
"""

import hashlib
import json
import math
import os
import struct
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path

os.environ.setdefault("TF_CPP_MIN_LOG_LEVEL", "2")
os.environ.setdefault("GLOG_minloglevel", "2")

import numpy as np  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "tmp-test" / "vision-convert"
DOWNLOADS = WORK / "downloads"
SRC = WORK / "src"
OUT = WORK / "out"
OPSET = 17

SOURCES = {
    "face_landmarker.task": (
        "https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1/face_landmarker.task",
        "64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff",
    ),
    "selfie_segmenter.tflite": (
        "https://storage.googleapis.com/mediapipe-models/image_segmenter/selfie_segmenter/float16/1/selfie_segmenter.tflite",
        "191ac9529ae506ee0beefa6b2c945a172dab9d07d1e802a290a4e4038226658b",
    ),
    # Public domain (NASA/NOAA), https://commons.wikimedia.org/wiki/File:NOAA_Interview_Opportunity-_Ready_to_GOES!_NOAA%E2%80%99s_Latest_GOES_Weather_Satellite_Ready_To_Launch_Next_Week!_(SVS14602_-_GOES-U_Canned_Side_Interview_Pam_Sullivan).webm
    "video/pam_sullivan.webm": (
        "https://upload.wikimedia.org/wikipedia/commons/4/42/NOAA_Interview_Opportunity-_Ready_to_GOES%21_NOAA%E2%80%99s_Latest_GOES_Weather_Satellite_Ready_To_Launch_Next_Week%21_%28SVS14602_-_GOES-U_Canned_Side_Interview_Pam_Sullivan%29.webm",
        "8074c56d64204ad9b44c0cde54ab6ea0ac04792f8a79f1b2b9bc8eccedf6facf",
    ),
    # Public domain (NASA), https://commons.wikimedia.org/wiki/File:NASA_Interview_Opportunity-_Ready_for_Takeoff_-_Trailblazing_Satellite_Will_Kick_Off_New_Era_Of_Earth_Observations_(SVS14844_-_Karen_St_Germain).webm
    "video/karen_st_germain.webm": (
        "https://upload.wikimedia.org/wikipedia/commons/a/af/NASA_Interview_Opportunity-_Ready_for_Takeoff_-_Trailblazing_Satellite_Will_Kick_Off_New_Era_Of_Earth_Observations_%28SVS14844_-_Karen_St_Germain%29.webm",
        "1a91b006ece101fbbb528e512204d78b2020ba70005f1a7e9a3240944833587f",
    ),
}

# Members of face_landmarker.task (a zip) and their hashes.
TASK_MEMBERS = {
    "face_landmarks_detector.tflite": "c7d54204ce0448474c7f3fa9af494787c0965cbdd6f20fc72867e43046bd43d5",
    "face_blendshapes.tflite": "4f36dded049db18d76048567439b2a7f58f1daabc00d78bfe8f3ad396a2d2082",
}

MODELS = {
    "face_landmarks_v2.onnx": {
        "tflite": SRC / "face_landmarks_detector.tflite",
        "source": "face_landmarker.task",
        "member": "face_landmarks_detector.tflite",
        "title": "MediaPipe Face Mesh V2 (face_landmarks_detector.tflite)",
        "model_card": "https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf",
        "rename": {
            "input_12": "image",
            "Identity": "landmarks",
            "Identity_1": "presence_logit",
            "Identity_2": "tongue_out",
        },
        "doc": (
            "image: float32 [1,256,256,3] NHWC RGB in [0,1] (pixel/255), a face crop made as in MediaPipe. "
            "landmarks: [1,1,1,1434] = 478 x (x,y,z); x,y in crop pixels (0..256), z in the same pixel scale. "
            "presence_logit: [1,1,1,1] raw logit, MediaPipe applies sigmoid and thresholds at 0.5. "
            "tongue_out: [1,1] probability (sigmoid already applied)."
        ),
    },
    "face_blendshapes_v2.onnx": {
        "tflite": SRC / "face_blendshapes.tflite",
        "source": "face_landmarker.task",
        "member": "face_blendshapes.tflite",
        "title": "MediaPipe Blendshape V2 (face_blendshapes.tflite)",
        "model_card": "https://storage.googleapis.com/mediapipe-assets/Model%20Card%20Blendshape%20V2.pdf",
        "rename": {
            "serving_default_input_points:0": "points",
            "StatefulPartitionedCall:0": "blendshapes",
        },
        "optimize_transpose": False,
        "doc": (
            "points: float32 [1,146,2], (x,y) in pixels of the 146-landmark subset of Face Mesh V2 (MediaPipe feeds "
            "full-image pixels: x*image_width, y*image_height). blendshapes: float32 [52] scores in [0,1], "
            "MediaPipe order starting with _neutral."
        ),
    },
    "selfie_segmenter.onnx": {
        "tflite": DOWNLOADS / "selfie_segmenter.tflite",
        "source": "selfie_segmenter.tflite",
        "member": None,
        "title": "MediaPipe Selfie Segmenter, square 256x256 (selfie_segmenter.tflite)",
        "model_card": "https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Selfie%20Segmentation.pdf",
        "rename": {"input_1": "image", "activation_10": "mask"},
        "doc": (
            "image: float32 [1,256,256,3] NHWC RGB in [0,1] (pixel/255), whole frame resized to 256x256. "
            "mask: float32 [1,256,256,1] person probability in [0,1] (sigmoid already applied)."
        ),
    },
}

# Parity thresholds (max absolute difference).
THRESHOLDS = {
    "face_landmarks_v2.onnx": {"landmarks": 0.05, "presence_prob": 1e-3, "tongue_out": 1e-3},
    "face_blendshapes_v2.onnx": {"blendshapes": 1e-3},
    "selfie_segmenter.onnx": {"mask": 1e-3},
}

# From mediapipe/tasks/cc/vision/face_landmarker/face_blendshapes_graph.cc
BLENDSHAPE_SUBSET = [
    0, 1, 4, 5, 6, 7, 8, 10, 13, 14, 17, 21, 33, 37, 39,
    40, 46, 52, 53, 54, 55, 58, 61, 63, 65, 66, 67, 70, 78, 80,
    81, 82, 84, 87, 88, 91, 93, 95, 103, 105, 107, 109, 127, 132, 133,
    136, 144, 145, 146, 148, 149, 150, 152, 153, 154, 155, 157, 158, 159, 160,
    161, 162, 163, 168, 172, 173, 176, 178, 181, 185, 191, 195, 197, 234, 246,
    249, 251, 263, 267, 269, 270, 276, 282, 283, 284, 285, 288, 291, 293, 295,
    296, 297, 300, 308, 310, 311, 312, 314, 317, 318, 321, 323, 324, 332, 334,
    336, 338, 356, 361, 362, 365, 373, 374, 375, 377, 378, 379, 380, 381, 382,
    384, 385, 386, 387, 388, 389, 390, 397, 398, 400, 402, 405, 409, 415, 454,
    466, 468, 469, 470, 471, 472, 473, 474, 475, 476, 477,
]
assert len(BLENDSHAPE_SUBSET) == 146

FRAME_STEP_S = 6.0
MAX_FACES_PER_VIDEO = 24


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch(name):
    url, digest = SOURCES[name]
    path = DOWNLOADS / name
    if path.exists() and sha256(path) == digest:
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    part = path.with_suffix(path.suffix + ".part")
    print(f"download {name}")
    req = urllib.request.Request(url, headers={"User-Agent": "nuzky-convert-mediapipe/1 (https://nuzky.app)"})
    with urllib.request.urlopen(req, timeout=60) as r, open(part, "wb") as f:
        while chunk := r.read(1 << 20):
            f.write(chunk)
    got = sha256(part)
    if got != digest:
        part.unlink()
        sys.exit(f"SHA-256 mismatch for {name}: expected {digest}, got {got}")
    part.replace(path)
    return path


def extract_task_members(task):
    SRC.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(task) as z:
        for member, digest in TASK_MEMBERS.items():
            data = z.read(member)
            got = hashlib.sha256(data).hexdigest()
            if got != digest:
                sys.exit(f"SHA-256 mismatch for {member} in {task.name}: expected {digest}, got {got}")
            (SRC / member).write_bytes(data)


# ---------------------------------------------------------------- conversion


def patch_tf2onnx():
    """Let tf2onnx push transposes through HardSwish (elementwise, missing from its table)."""
    from tf2onnx.optimizer.transpose_optimizer import TransposeOptimizer

    orig = TransposeOptimizer._initialize_handlers

    def init(self):
        orig(self)
        self._handler_map.setdefault("HardSwish", self._simple_through_handler)

    TransposeOptimizer._initialize_handlers = init


def read_tflite(path):
    from tensorflow.lite.python import schema_py_generated as tfl

    buf = Path(path).read_bytes()
    return tfl.ModelT.InitFromObj(tfl.Model.GetRootAsModel(buf, 0))


def replace_conv2d_transpose_bias(model, tflite_path):
    """Swap MediaPipe's custom Convolution2DTransposeBias for a standard ConvTranspose.

    Semantics (mediapipe/util/tflite/operations/transpose_conv_bias.cc): inputs are
    data NHWC, weights OHWI, bias [O]; custom options are TfLiteTransposeConvParams
    {padding, stride_width, stride_height}; output = bias + transposed convolution,
    with SAME padding total = max(0, k - (in - 1) % s - 1), split floor(total/2)
    before and the rest after.
    """
    from onnx import helper, numpy_helper

    tm = read_tflite(tflite_path)
    sg = tm.subgraphs[0]
    params = {}
    for op in sg.operators:
        code = tm.operatorCodes[op.opcodeIndex]
        if code.customCode == b"Convolution2DTransposeBias":
            padding, stride_w, stride_h = struct.unpack("<iii", bytes(op.customOptions)[:12])
            data_t, w_t = sg.tensors[op.inputs[0]], sg.tensors[op.inputs[1]]
            out_name = sg.tensors[op.outputs[0]].name.decode()
            params[out_name] = (padding, stride_w, stride_h, list(data_t.shape), list(w_t.shape))

    g = model.graph
    inits = {i.name: i for i in g.initializer}
    replaced = 0
    for node in list(g.node):
        if node.op_type != "TFL_Convolution2DTransposeBias":
            continue
        padding, sw, sh, in_shape, w_shape = params[node.output[0]]
        if padding not in (1, 2):  # kTfLitePaddingSame, kTfLitePaddingValid
            raise ValueError(f"unexpected padding {padding}")
        _, in_h, in_w, _ = in_shape
        _, kh, kw, _ = w_shape
        pad_h = max(0, kh - (in_h - 1) % sh - 1) if padding == 1 else 0
        pad_w = max(0, kw - (in_w - 1) % sw - 1) if padding == 1 else 0
        data, w_name, b_name = node.input
        weights = numpy_helper.to_array(inits[w_name]).astype(np.float32)  # OHWI
        bias = numpy_helper.to_array(inits[b_name]).astype(np.float32)
        base = node.output[0]
        new_w = numpy_helper.from_array(np.ascontiguousarray(weights.transpose(3, 0, 1, 2)), base + "/convtranspose_W")
        new_b = numpy_helper.from_array(bias, base + "/convtranspose_B")
        nodes = [
            helper.make_node("Transpose", [data], [base + "/nchw_in"], name=base + "/to_nchw", perm=[0, 3, 1, 2]),
            helper.make_node(
                "ConvTranspose",
                [base + "/nchw_in", new_w.name, new_b.name],
                [base + "/nchw_out"],
                name=base + "/convtranspose",
                kernel_shape=[kh, kw],
                strides=[sh, sw],
                pads=[pad_h // 2, pad_w // 2, pad_h - pad_h // 2, pad_w - pad_w // 2],
            ),
            helper.make_node("Transpose", [base + "/nchw_out"], [base], name=base + "/to_nhwc", perm=[0, 2, 3, 1]),
        ]
        pos = list(g.node).index(node)
        g.node.remove(node)
        for k, n in enumerate(nodes):
            g.node.insert(pos + k, n)
        g.initializer.extend([new_w, new_b])
        replaced += 1
    used = {i for n in g.node for i in n.input}
    for init in list(g.initializer):
        if init.name not in used:
            g.initializer.remove(init)
    if replaced != len(params) or replaced == 0:
        raise RuntimeError(f"replaced {replaced} of {len(params)} Convolution2DTransposeBias ops")
    return model


def simplify_reshapes(model):
    """Drop Reshapes whose static input shape already equals the target and merge
    Reshape(Reshape(x)). tf2onnx leaves a varying number of these behind (its optimizer
    iterates over sets of node objects), so removing them also makes the output stable."""
    from onnx import numpy_helper, shape_inference

    del model.graph.value_info[:]
    while True:
        inferred = shape_inference.infer_shapes(model, strict_mode=True).graph
        shapes = {}
        for v in list(inferred.value_info) + list(inferred.input) + list(inferred.output):
            dims = v.type.tensor_type.shape.dim
            shapes[v.name] = [d.dim_value if d.HasField("dim_value") else None for d in dims]
        g = model.graph
        inits = {i.name: numpy_helper.to_array(i) for i in g.initializer}
        graph_outputs = {o.name for o in g.output}
        producer = {o: n for n in g.node for o in n.output}
        uses = {}
        for n in g.node:
            for x in n.input:
                uses[x] = uses.get(x, 0) + 1
        changed = False
        for n in g.node:
            if n.op_type != "Reshape" or n.input[1] not in inits:
                continue
            target = inits[n.input[1]].tolist()
            src, dst = n.input[0], n.output[0]
            if 0 in target:
                continue
            if shapes.get(src) == target and dst not in graph_outputs:
                for m in g.node:
                    m.input[:] = [src if x == dst else x for x in m.input]
                g.node.remove(n)
                changed = True
                break
            prev = producer.get(src)
            if prev is not None and prev.op_type == "Reshape" and uses.get(src) == 1 and src not in graph_outputs:
                n.input[0] = prev.input[0]
                g.node.remove(prev)
                changed = True
                break
        if not changed:
            return model


def canonicalize_names(model):
    """Name nodes, tensors and weights by graph position so the file does not depend on
    tf2onnx's internal name counters. Graph inputs and outputs keep their names."""
    g = model.graph
    keep = {v.name for v in g.input} | {v.name for v in g.output}
    inits = {i.name: i for i in g.initializer}
    rename, ordered = {}, []
    for i, n in enumerate(g.node):
        for x in n.input:
            if x in inits and x not in rename:
                rename[x] = f"w{len(ordered)}"
                ordered.append(inits[x])
        for k, y in enumerate(n.output):
            if y and y not in keep:
                rename[y] = f"{n.op_type.lower()}_{i}" + (f"_{k}" if k else "")
        n.name = f"{n.op_type}_{i}"
    for n in g.node:
        n.input[:] = [rename.get(x, x) for x in n.input]
        n.output[:] = [rename.get(x, x) for x in n.output]
    for init in ordered:
        init.name = rename[init.name]
    del g.initializer[:]
    g.initializer.extend(ordered)
    del g.value_info[:]
    return model


def finalize(model, name, spec, tflite_io):
    import onnx
    import tensorflow as tf
    import tf2onnx
    from onnx import helper

    g = model.graph
    mapping = spec["rename"]
    for n in g.node:
        n.input[:] = [mapping.get(x, x) for x in n.input]
        n.output[:] = [mapping.get(x, x) for x in n.output]
    for v in list(g.input) + list(g.output):
        v.name = mapping.get(v.name, v.name)
    # Fix IO shapes to the TFLite shapes (tf2onnx makes some batch dims symbolic).
    for v in list(g.input) + list(g.output):
        dims = tflite_io[v.name]
        shape = v.type.tensor_type.shape
        del shape.dim[:]
        for d in dims:
            shape.dim.add().dim_value = int(d)
    del g.value_info[:]
    used_domains = {n.domain for n in g.node}
    keep = [o for o in model.opset_import if o.domain in ("", "ai.onnx") or o.domain in used_domains]
    del model.opset_import[:]
    model.opset_import.extend(keep)
    model = onnx.shape_inference.infer_shapes(model, strict_mode=True)
    src_url, src_sha = SOURCES[spec["source"]]
    props = {
        "title": spec["title"],
        "license": "Apache-2.0",
        "model_card": spec["model_card"],
        "source_url": src_url,
        "source_sha256": src_sha,
        "converted_with": f"scripts/convert-mediapipe.py, tf2onnx {tf2onnx.__version__}, tensorflow {tf.__version__}, opset {OPSET}",
        "io": spec["doc"],
    }
    if spec["member"]:
        props["source_member"] = spec["member"]
        props["source_member_sha256"] = TASK_MEMBERS[spec["member"]]
    helper.set_model_props(model, props)
    model.doc_string = f"{spec['title']}. Converted from TFLite, Apache-2.0. {spec['doc']}"
    return model


def tflite_io_shapes(spec):
    tm = read_tflite(spec["tflite"])
    sg = tm.subgraphs[0]
    shapes = {}
    for t in list(sg.inputs) + list(sg.outputs):
        tensor = sg.tensors[t]
        shapes[spec["rename"][tensor.name.decode()]] = list(tensor.shape)
    return shapes


def convert(name, spec):
    import onnx
    import tf2onnx
    from tf2onnx import optimizer
    from tf2onnx.graph import GraphUtil

    # tf2onnx numbers generated node names with a process-global counter; reset it so the
    # output does not depend on what was converted before in this process.
    tf2onnx.utils.INTERNAL_NAME = 1
    optimizers = dict(optimizer._optimizers)
    if not spec.get("optimize_transpose", True):
        # On the blendshape MLP-Mixer this pass gives a different graph on every run.
        del optimizers["optimize_transpose"]
    # Same as tf2onnx.convert.from_tflite, plus the optimizer list.
    model, _ = tf2onnx.convert._convert_common(
        None, tflite_path=str(spec["tflite"]), name=Path(spec["tflite"]).stem, continue_on_error=True,
        opset=OPSET, optimizers=optimizers, input_names=None, output_names=None, large_model=False,
        output_path=None, tensors_to_rename=None, initialized_tables=None,
    )
    if any(n.op_type == "TFL_Convolution2DTransposeBias" for n in model.graph.node):
        model = replace_conv2d_transpose_bias(model, spec["tflite"])
        model = GraphUtil.optimize_model_proto(model, catch_errors=False)
    unknown = sorted({n.op_type for n in model.graph.node if n.op_type.startswith("TFL_") or n.domain not in ("", "ai.onnx")})
    if unknown:
        raise RuntimeError(f"{name}: unconverted ops {unknown}")
    model = simplify_reshapes(model)
    model = canonicalize_names(model)
    model = finalize(model, name, spec, tflite_io_shapes(spec))
    onnx.checker.check_model(model, full_check=True)
    return model


def convert_all():
    import onnx

    patch_tf2onnx()
    OUT.mkdir(parents=True, exist_ok=True)
    summary = {}
    for name, spec in MODELS.items():
        runs = [convert(name, spec).SerializeToString() for _ in range(3)]
        deterministic = all(r == runs[0] for r in runs)
        (OUT / name).write_bytes(runs[0])
        m = onnx.load_from_string(runs[0])
        ops = {}
        for n in m.graph.node:
            ops[n.op_type] = ops.get(n.op_type, 0) + 1
        summary[name] = {"deterministic": deterministic, "ops": ops}
        print(f"converted {name}: identical over 3 conversions={deterministic}, ops={ops}")
    return summary


# ---------------------------------------------------------------- inputs


def probe(video):
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height:format=duration", "-of", "json", str(video)],
        check=True, capture_output=True, text=True,
    ).stdout
    info = json.loads(out)
    s = info["streams"][0]
    return s["width"], s["height"], float(info["format"]["duration"])


def grab(video, t, w, h):
    raw = subprocess.run(
        ["ffmpeg", "-v", "error", "-ss", f"{t:.3f}", "-i", str(video), "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
        check=True, capture_output=True,
    ).stdout
    if len(raw) != w * h * 3:
        return None
    return np.frombuffer(raw, np.uint8).reshape(h, w, 3).copy()


def normalize_radians(a):
    return a - 2 * math.pi * math.floor((a + math.pi) / (2 * math.pi))


def rect_from_landmarks(lm, w, h, scale=1.5):
    """MediaPipe's tracking rect: LandmarksToDetection -> DetectionsToRects (rotation from
    landmark 33 to 263, target 0) -> RectTransformation (square_long, scale 1.5).
    lm: (478, 3) normalized. Returns (cx, cy, width, height, rotation) in pixels/radians."""
    xmin, xmax = lm[:, 0].min(), lm[:, 0].max()
    ymin, ymax = lm[:, 1].min(), lm[:, 1].max()
    cx, cy = (xmin + xmax) / 2 * w, (ymin + ymax) / 2 * h
    x0, y0 = lm[33, 0] * w, lm[33, 1] * h
    x1, y1 = lm[263, 0] * w, lm[263, 1] * h
    rotation = normalize_radians(0.0 - math.atan2(-(y1 - y0), x1 - x0))
    long_side = max((xmax - xmin) * w, (ymax - ymin) * h)
    return cx, cy, long_side * scale, long_side * scale, rotation


def crop(image, rect, size=256):
    """MediaPipe ImageToTensor CPU path: cv::boxPoints + getPerspectiveTransform + warpPerspective,
    INTER_LINEAR, BORDER_REPLICATE, no letterbox (keep_aspect_ratio false)."""
    import cv2

    cx, cy, rw, rh, rot = rect
    src = cv2.boxPoints(((cx, cy), (rw, rh), rot * 180.0 / math.pi)).astype(np.float32)
    dst = np.array([[0, size], [0, 0], [size, 0], [size, size]], np.float32)
    m = cv2.getPerspectiveTransform(src, dst)
    return cv2.warpPerspective(image, m, (size, size), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)


def project(lm_crop_px, rect, w, h, size=256):
    """LandmarkProjectionCalculator: crop pixels -> image pixels."""
    cx, cy, rw, rh, rot = rect
    u = lm_crop_px[:, 0] / size - 0.5
    v = lm_crop_px[:, 1] / size - 0.5
    c, s = math.cos(rot), math.sin(rot)
    x = cx + c * rw * u - s * rh * v
    y = cy + s * rw * u + c * rh * v
    return np.stack([x, y], axis=1)


def collect_faces():
    import mediapipe as mp

    task = DOWNLOADS / "face_landmarker.task"
    opts = mp.tasks.vision.FaceLandmarkerOptions(
        base_options=mp.tasks.BaseOptions(model_asset_path=str(task)),
        running_mode=mp.tasks.vision.RunningMode.IMAGE,
        num_faces=1,
        output_face_blendshapes=True,
    )
    faces, frames = [], []
    with mp.tasks.vision.FaceLandmarker.create_from_options(opts) as lmk:
        for name in ("video/pam_sullivan.webm", "video/karen_st_germain.webm"):
            video = DOWNLOADS / name
            w, h, duration = probe(video)
            kept = 0
            for t in np.arange(3.0, duration - 1.0, FRAME_STEP_S):
                image = grab(video, float(t), w, h)
                if image is None:
                    continue
                frames.append({"video": name, "t": float(t), "image": image})
                if kept >= MAX_FACES_PER_VIDEO:
                    continue
                res = lmk.detect(mp.Image(image_format=mp.ImageFormat.SRGB, data=image))
                if not res.face_landmarks:
                    continue
                lm = np.array([[p.x, p.y, p.z] for p in res.face_landmarks[0]], np.float64)
                bs = np.array([c.score for c in res.face_blendshapes[0]], np.float32)
                rect = rect_from_landmarks(lm, w, h)
                faces.append({
                    "video": name, "t": float(t), "image": image, "w": w, "h": h,
                    "mp_landmarks": lm, "mp_blendshapes": bs, "rect": rect, "crop": crop(image, rect),
                })
                kept += 1
    return faces, frames


def extra_crops(faces):
    """Imperfect crops (other scales, shift, extra roll) and synthetic images."""
    rng = np.random.default_rng(1234)
    out = []
    for i, f in enumerate(faces[:6]):
        cx, cy, rw, rh, rot = f["rect"]
        variants = [
            (cx, cy, rw * 0.8, rh * 0.8, rot),
            (cx, cy, rw * 1.3, rh * 1.3, rot),
            (cx + 0.12 * rw, cy - 0.08 * rh, rw, rh, rot),
            (cx, cy, rw, rh, rot + math.radians(25 if i % 2 else -25)),
        ]
        out.append(crop(f["image"], variants[i % len(variants)]))
    out.append(rng.integers(0, 256, (256, 256, 3), dtype=np.uint8))
    out.append(rng.integers(0, 256, (256, 256, 3), dtype=np.uint8))
    out.append(np.zeros((256, 256, 3), np.uint8))
    out.append(np.full((256, 256, 3), 255, np.uint8))
    return out


# ---------------------------------------------------------------- parity


class Stats:
    def __init__(self):
        self.max = 0.0
        self.sum = 0.0
        self.n = 0

    def add(self, a, b):
        d = np.abs(np.asarray(a, np.float64) - np.asarray(b, np.float64))
        self.max = max(self.max, float(d.max()))
        self.sum += float(d.sum())
        self.n += d.size

    def report(self):
        return {"max_abs": self.max, "mean_abs": self.sum / max(self.n, 1), "values": self.n}


def litert(path):
    from ai_edge_litert.interpreter import Interpreter, OpResolverType

    it = Interpreter(model_path=str(path), experimental_op_resolver_type=OpResolverType.BUILTIN_WITHOUT_DEFAULT_DELEGATES)
    it.allocate_tensors()
    ins, outs = it.get_input_details(), it.get_output_details()

    def run(x):
        it.set_tensor(ins[0]["index"], x)
        it.invoke()
        return {d["name"]: it.get_tensor(d["index"]).copy() for d in outs}

    return run


def onnx_session(path):
    import onnxruntime as ort

    sess = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    inp = sess.get_inputs()[0].name
    names = [o.name for o in sess.get_outputs()]

    def run(x):
        return dict(zip(names, sess.run(None, {inp: x}), strict=True))

    return run


def sigmoid(x):
    return 1.0 / (1.0 + np.exp(-np.asarray(x, np.float64)))


def parity(faces, frames):
    import mediapipe as mp

    results = {}
    crops = [f["crop"] for f in faces]
    extras = extra_crops(faces)
    mesh_inputs = crops + extras

    # Face Mesh V2: LiteRT (original fp16-weight TFLite) vs ONNX Runtime.
    spec = MODELS["face_landmarks_v2.onnx"]
    ren = spec["rename"]
    tfl, onx = litert(spec["tflite"]), onnx_session(OUT / "face_landmarks_v2.onnx")
    st = {k: Stats() for k in ("landmarks", "landmarks_xy", "presence_logit", "presence_prob", "tongue_out")}
    tfl_landmarks = []
    for img in mesh_inputs:
        x = (img.astype(np.float32) / 255.0)[None]
        a = {ren[k]: v for k, v in tfl(x).items()}
        b = onx(x)
        la, lb = a["landmarks"].reshape(478, 3), b["landmarks"].reshape(478, 3)
        st["landmarks"].add(la, lb)
        st["landmarks_xy"].add(la[:, :2], lb[:, :2])
        st["presence_logit"].add(a["presence_logit"], b["presence_logit"])
        st["presence_prob"].add(sigmoid(a["presence_logit"]), sigmoid(b["presence_logit"]))
        st["tongue_out"].add(a["tongue_out"], b["tongue_out"])
        tfl_landmarks.append(la)
    results["face_landmarks_v2.onnx"] = {
        "inputs": f"{len(crops)} real face crops + {len(extras)} perturbed/synthetic",
        **{k: s.report() for k, s in st.items()},
    }

    # Convention check: our crop + ONNX + projection vs MediaPipe's own landmarks (it used the
    # detector's crop, so small differences are expected; wrong conventions would be far off).
    errs = []
    onnx_mesh = []
    for f in faces:
        x = (f["crop"].astype(np.float32) / 255.0)[None]
        lm = onx(x)["landmarks"].reshape(478, 3)
        onnx_mesh.append(lm)
        ours = project(lm[:, :2], f["rect"], f["w"], f["h"])
        theirs = f["mp_landmarks"][:, :2] * [f["w"], f["h"]]
        iod = np.linalg.norm(theirs[33] - theirs[263])
        errs.append(np.linalg.norm(ours - theirs, axis=1).mean() / iod)
    results["convention_check_face_mesh"] = {
        "mean_landmark_error_over_eye_corner_distance": float(np.mean(errs)),
        "max_per_face": float(np.max(errs)),
        "presence_prob_min_on_real_faces": float(min(sigmoid(onx((f["crop"].astype(np.float32) / 255.0)[None])["presence_logit"]).item() for f in faces)),
    }

    # Blendshape V2: LiteRT vs ONNX Runtime.
    spec = MODELS["face_blendshapes_v2.onnx"]
    ren = spec["rename"]
    tfl, onx_b = litert(spec["tflite"]), onnx_session(OUT / "face_blendshapes_v2.onnx")
    rng = np.random.default_rng(99)
    bs_inputs = []
    for f, lm in zip(faces, tfl_landmarks[: len(faces)], strict=True):
        bs_inputs.append(lm[BLENDSHAPE_SUBSET, :2])  # crop pixels
        bs_inputs.append(f["mp_landmarks"][BLENDSHAPE_SUBSET, :2] * [f["w"], f["h"]])  # image pixels, as MediaPipe
    for lm in tfl_landmarks[:10]:
        bs_inputs.append(lm[BLENDSHAPE_SUBSET, :2] + rng.normal(0, 3.0, (146, 2)))
    st = Stats()
    for pts in bs_inputs:
        x = pts.astype(np.float32)[None]
        a = {ren[k]: v for k, v in tfl(x).items()}["blendshapes"]
        st.add(a, onx_b(x)["blendshapes"])
    results["face_blendshapes_v2.onnx"] = {
        "inputs": f"{len(bs_inputs)} point sets (crop-pixel and image-pixel landmarks of {len(faces)} faces, 10 noisy)",
        "blendshapes": st.report(),
    }

    # Convention check: MediaPipe's landmarks fed as image pixels must reproduce MediaPipe's
    # own blendshape scores; crop pixels vs image pixels shows how much the frame matters.
    conv, frame_dep = Stats(), Stats()
    for f, lm in zip(faces, onnx_mesh, strict=True):
        pts = (f["mp_landmarks"][BLENDSHAPE_SUBSET, :2] * [f["w"], f["h"]]).astype(np.float32)[None]
        conv.add(onx_b(pts)["blendshapes"], f["mp_blendshapes"])
        crop_pts = lm[BLENDSHAPE_SUBSET, :2].astype(np.float32)[None]
        img_pts = project(lm[BLENDSHAPE_SUBSET, :2], f["rect"], f["w"], f["h"]).astype(np.float32)[None]
        frame_dep.add(onx_b(crop_pts)["blendshapes"], onx_b(img_pts)["blendshapes"])
    results["convention_check_blendshapes"] = {
        "onnx_on_mediapipe_landmarks_vs_mediapipe_scores": conv.report(),
        "crop_pixels_vs_image_pixels_same_landmarks": frame_dep.report(),
    }

    # Selfie Segmenter: MediaPipe ImageSegmenter (runs the original TFLite with MediaPipe's own
    # Convolution2DTransposeBias kernel; plain LiteRT cannot load that custom op) vs ONNX Runtime.
    import cv2

    seg_inputs = [cv2.resize(fr["image"], (256, 256), interpolation=cv2.INTER_AREA) for fr in frames[::8]]
    seg_inputs += crops[:20] + extras[-4:]
    opts = mp.tasks.vision.ImageSegmenterOptions(
        base_options=mp.tasks.BaseOptions(model_asset_path=str(DOWNLOADS / "selfie_segmenter.tflite")),
        running_mode=mp.tasks.vision.RunningMode.IMAGE,
        output_confidence_masks=True,
        output_category_mask=False,
    )
    onx_s = onnx_session(OUT / "selfie_segmenter.onnx")
    st = Stats()
    with mp.tasks.vision.ImageSegmenter.create_from_options(opts) as seg:
        for img in seg_inputs:
            img = np.ascontiguousarray(img)
            ref = seg.segment(mp.Image(image_format=mp.ImageFormat.SRGB, data=img)).confidence_masks[0].numpy_view()
            ours = onx_s((img.astype(np.float32) / 255.0)[None])["mask"]
            st.add(ref.reshape(256, 256), ours.reshape(256, 256))
    results["selfie_segmenter.onnx"] = {
        "inputs": f"{len(seg_inputs)} images (whole frames resized to 256x256, face crops, synthetic)",
        "reference": "MediaPipe 1.1.0 ImageSegmenter confidence mask on the original TFLite",
        "mask": st.report(),
    }
    return results, mesh_inputs


def contact_sheet(mesh_inputs, path):
    import cv2

    onx = onnx_session(OUT / "face_landmarks_v2.onnx")
    tiles = []
    for img in mesh_inputs[:40]:
        lm = onx((img.astype(np.float32) / 255.0)[None])["landmarks"].reshape(478, 3)
        tile = cv2.cvtColor(img, cv2.COLOR_RGB2BGR)
        for x, y, _ in lm:
            cv2.circle(tile, (int(round(x)), int(round(y))), 1, (0, 255, 0), -1)
        tiles.append(cv2.resize(tile, (160, 160), interpolation=cv2.INTER_AREA))
    while len(tiles) % 8:
        tiles.append(np.zeros_like(tiles[0]))
    rows = [np.hstack(tiles[i : i + 8]) for i in range(0, len(tiles), 8)]
    cv2.imwrite(str(path), np.vstack(rows))


def check(results):
    failed = []
    for name, limits in THRESHOLDS.items():
        for key, limit in limits.items():
            got = results[name][key]["max_abs"]
            if not got <= limit:
                failed.append(f"{name} {key}: max |diff| {got:.3g} > {limit}")
    return failed


def main():
    # tf2onnx's optimizer iterates over sets of strings, so string hash randomization
    # changes the graph between runs. A fixed seed makes the output reproducible.
    if os.environ.get("PYTHONHASHSEED") != "0":
        os.execve(sys.executable, [sys.executable, *sys.argv], {**os.environ, "PYTHONHASHSEED": "0"})
    if "tmp-test/vision-convert" not in os.environ.get("UV_CACHE_DIR", "").replace("\\", "/"):
        print("warning: UV_CACHE_DIR is not under tmp-test/vision-convert; see the docstring for the command")
    for tool in ("ffmpeg", "ffprobe"):
        subprocess.run([tool, "-version"], check=True, capture_output=True)
    for name in SOURCES:
        fetch(name)
    extract_task_members(DOWNLOADS / "face_landmarker.task")

    conversion = convert_all()
    faces, frames = collect_faces()
    if len(faces) < 20:
        sys.exit(f"only {len(faces)} faces found, need at least 20")
    results, mesh_inputs = parity(faces, frames)
    contact_sheet(mesh_inputs, WORK / "work" / "crops_with_onnx_landmarks.jpg")
    results["conversion"] = conversion

    files = {}
    for name in MODELS:
        p = OUT / name
        files[name] = {"bytes": p.stat().st_size, "sha256": sha256(p)}
    results["files"] = files
    (OUT / "parity.json").write_text(json.dumps(results, indent=2) + "\n")

    print("\nparity (max / mean absolute difference)")
    for name in MODELS:
        r = results[name]
        print(f"  {name}: {r['inputs']}")
        for key, val in r.items():
            if isinstance(val, dict) and "max_abs" in val:
                print(f"    {key:16s} max {val['max_abs']:.3e}  mean {val['mean_abs']:.3e}  ({val['values']} values)")
    print("  convention checks:")
    print("    " + json.dumps(results["convention_check_face_mesh"]))
    print("    " + json.dumps(results["convention_check_blendshapes"]))
    print("\noutputs")
    for name, info in files.items():
        print(f"  {name}  {info['bytes']} bytes  sha256 {info['sha256']}")

    failed = check(results)
    if failed or not all(c["deterministic"] for c in conversion.values()):
        print("\nFAILED:\n  " + "\n  ".join(failed or ["conversion is not deterministic"]))
        sys.exit(1)
    print("\nOK: all outputs within thresholds")


if __name__ == "__main__":
    main()
