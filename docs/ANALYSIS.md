# Helicon Focus 8 – analysis notes

Target: `C:\Program Files\Helicon Software\Helicon Focus 8\HeliconFocus.exe`
(8.3.11.0, x86-64 PE, 16.2 MB, SHA-256 `29ca62df122163c1295e1e2a416a42acfdf52f20c83d0beed99c4af154d42089`).

Method: REA (`open_binary`, `inspect_pe_resources` – evidence
`ev_0400e87bb939711217c49250f3d823c8c36c8f5a1762a7005c2dcc821a771f5d`) plus a
static string/RTTI-name extraction of the executable. No source code or
proprietary resources were extracted or reused.

## What REA could and could not do here

REA's deep-analysis providers (Ghidra, IDA, Kuna) were not configured on this
machine and Hopper is unsupported on Windows, so **no disassembly or
decompilation was performed**. Everything below is *observed* from strings,
RTTI class names and PE resources, or *inferred* where stated.

## Findings

### 1. Helicon Focus does not talk to cameras itself  (observed)

* No camera-SDK or USB-camera strings exist in `HeliconFocus.exe` (searched
  for EDSDK, libgphoto2, libusb, WIA, WPD, PTP, CRSDK, MAID, live view,
  tether). The rest of the install folder consists, by file name, of Qt 6, the
  MSVC/UCRT runtime, OpenSSL, an `ffmpeg` folder and `Helicon3DViewer.exe`;
  those were **not** string-searched, so this negative result is bounded to the
  main executable.
* Camera-related text is limited to **metadata decoding**: Exiv2 maker-note
  tables (Canon/Nikon/Sony/Olympus/Pentax/Panasonic/Fujifilm) and LibRaw (RAW
  decoding, camera list).
* Camera control is **delegated to a separate product, Helicon Remote**:
  `actionHeliconRemote`, `MainWindowController::onHeliconRemote`,
  `heliconRemotePath()`, setting `HeliconRemoteFilePath`, registry key
  `SOFTWARE\Helicon\Remote`, log lines "Helicon Remote path found in
  registry/settings", "Helicon Remote requested by the user", "Failed to launch
  Helicon Remote from", the prompt "Helicon Remote not found. Do you want to
  download it?" and a download URL on heliconsoft.com.

### 2. The hand-off between the two programs  (observed + inferred)

* Launch: resolve the Remote path from the registry, then from the saved
  setting; if missing, offer to download; otherwise start the process.
* Data flow (inferred from the strings below): shots taken by Remote land in a
  folder; Focus's **Folder Monitor** (`FolderMonitorDialog`,
  `FileSystemWatcher`, `FolderMonitorStatusListener`, "Drive or folder to
  monitor", "Destination copy folder cannot be a subfolder of monitor folder")
  and **Enqueue stacks** (`EnqueueStacksDialog`, `SplitStackDialog`) detect and
  group new images into stacks automatically.

### 3. Stacking engine  (observed class names, behaviour inferred)

RTTI shows `HF_Stacking` with `HF_AverageStacking`, `HF_WeightedStacking`,
`HF_DepthMapStacking`, `HF_PyramidStacking` and `HF_DepthMapPyramidStacking`,
running tasks through PPL with per-image alignment parameters
(`HLAlignmentParams`) and an image pyramid type (`HLImagePyramid`). This only
informed the *shape* of Lumen's stacker (per-pixel focus weights, weighted vs.
winner-takes-all selection); no algorithm was copied.

### 4. Not analysed

Helicon Remote itself (not installed; not downloaded because that was outside
the request) and any physical camera (none attached). Therefore the following
were **not** observed and are design choices in Lumen: how Remote enumerates
cameras, which SDKs it uses per vendor, how it streams live view and how it
maps UI controls to camera properties.

## How the findings shaped Lumen

| Observation | Lumen decision |
|---|---|
| Focus delegates tethering to a separate tool and hands over files via a watched folder | Lumen implements the tethering side itself and writes numbered sequences into per-bracket folders, then stacks them in-process |
| Stacks are produced by weighted / depth-map style merges | `imaging::stack` offers *Weighted* and *Sharpest* strategies built on smoothed Laplacian focus maps |
| Capture → enqueue → stack pipeline | Bracket → auto-stack toggle |
| Camera access method unknown | Standard ISO 15740 PTP over three interchangeable links (raw USB, Windows WPD, in-process simulator) with a vendor layer on top |
