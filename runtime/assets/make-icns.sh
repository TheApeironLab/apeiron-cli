#!/bin/bash
# Build cli/assets/apeiron.icns from the brand logo.
#
# The icon exists so the background service is recognisable where macOS shows
# it to the user: 系统设置 → 通用 → 登录项与扩展. Without a bundle carrying an
# icon, that row is a generic grey "exec" block.
#
# Run this only when the brand mark changes; the .icns is committed so a
# release build needs nothing but the repository. Requires macOS (iconutil,
# swift, sips) and Python with Pillow.
#
#   ./make-icns.sh
#
# What it does, and why each step is not the obvious one:
#
#  - The logo is read from frontend/fe-apeiron-app/public/logo/logo.svg,
#    the one the product ships, and it is the full lockup: the two-ring mark stacked over the word
#    APEIRON. At 16pt — the size that actually matters in 登录项与扩展 — the
#    word is three grey pixels of mud, so the icon uses the mark alone. The
#    mark is isolated by viewBox rather than by deleting paths: logo.svg is
#    auto-traced into 30 anonymous <path> elements, and guessing which ones
#    spell the word would break the next time the file is re-exported.
#
#  - The white 1024x1024 background plate (the first path) is dropped so the
#    icon is transparent. A white square looks like a bug on a dark Dock.
#
#  - macOS icons are not drawn edge to edge; the grid reserves a margin. This
#    mark is wide and flat, so it is scaled by area rather than by width —
#    see the note at that step for what that trades away.
set -euo pipefail

cd "$(dirname "$0")"

command -v iconutil >/dev/null || { echo "iconutil not found (macOS only)" >&2; exit 1; }
command -v swift >/dev/null || { echo "swift not found (install Xcode CLT)" >&2; exit 1; }
python3 -c 'import PIL' 2>/dev/null || { echo "Pillow not installed: pip3 install Pillow" >&2; exit 1; }

# The brand logo has one home, in the app that ships it; copying it here
# would be a second copy to keep in step with the first.
logo="../../frontend/fe-apeiron-app/public/logo/logo.svg"
[ -f "$logo" ] || { echo "cannot find $logo" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# 1. The mark alone, on a transparent canvas, as SVG.
#    85.5 210.5 852 425.5 is the mark's bounding box in logo.svg's 1024 grid,
#    measured from a render rather than guessed.
python3 - "$work" "$logo" <<'PY'
import re, sys, pathlib
work = pathlib.Path(sys.argv[1])
svg = pathlib.Path(sys.argv[2]).read_text()
paths = re.findall(r'<path\b[^>]*/>', svg)
if len(paths) < 2:
    raise SystemExit(f"logo.svg: expected many paths, found {len(paths)}")
# paths[0] is the full-canvas white plate; everything after it is artwork.
if 'M0 0 C337.92 0' not in paths[0]:
    raise SystemExit("logo.svg: first path is no longer the white background plate; re-check the crop")
body = "\n".join(paths[1:])
(work / "mark.svg").write_text(
    '<?xml version="1.0" encoding="UTF-8"?>\n'
    '<svg version="1.1" xmlns="http://www.w3.org/2000/svg" '
    'width="1704" height="851" viewBox="85.5 210.5 852 425.5">\n'
    f'{body}\n</svg>\n'
)
PY

# 2. Rasterise it. WebKit is the only SVG renderer guaranteed to be on a Mac,
#    so drive it rather than depend on rsvg/inkscape being installed.
cat > "$work/render.swift" <<'SWIFT'
import WebKit
import AppKit
let args = CommandLine.arguments
let svg = URL(fileURLWithPath: args[1]), out = URL(fileURLWithPath: args[2])
let w = Double(args[3])!, h = Double(args[4])!
let app = NSApplication.shared
let wv = WKWebView(frame: NSRect(x: 0, y: 0, width: w, height: h), configuration: WKWebViewConfiguration())
wv.setValue(false, forKey: "drawsBackground")   // keep the canvas transparent
wv.loadFileURL(svg, allowingReadAccessTo: svg.deletingLastPathComponent())
final class Done: NSObject, WKNavigationDelegate {
    var out: URL!, w: Double = 0, h: Double = 0
    func webView(_ view: WKWebView, didFinish navigation: WKNavigation!) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.7) {
            let cfg = WKSnapshotConfiguration()
            cfg.rect = NSRect(x: 0, y: 0, width: self.w, height: self.h)
            view.takeSnapshot(with: cfg) { image, _ in
                guard let image, let tiff = image.tiffRepresentation,
                      let rep = NSBitmapImageRep(data: tiff),
                      let png = rep.representation(using: .png, properties: [:])
                else { exit(1) }
                try? png.write(to: self.out)
                exit(0)
            }
        }
    }
}
let done = Done(); done.out = out; done.w = w; done.h = h
wv.navigationDelegate = done
app.run()
SWIFT
swift "$work/render.swift" "$work/mark.svg" "$work/mark.png" 1704 851 >/dev/null

# 3. Trim to the artwork and place it in the icon grid's content box.
python3 - "$work" <<'PY'
import sys, pathlib
from PIL import Image
work = pathlib.Path(sys.argv[1])
mark = Image.open(work / "mark.png").convert("RGBA")

# Drop near-white pixels the renderer may have left, then trim to the artwork.
pixels = mark.load()
for y in range(mark.height):
    for x in range(mark.width):
        r, g, b, a = pixels[x, y]
        if a < 8 or (r > 246 and g > 246 and b > 246):
            pixels[x, y] = (r, g, b, 0)
box = mark.getbbox()
if box is None:
    raise SystemExit("rendered mark is empty")
mark = mark.crop(box)

# macOS reserves a margin around icon artwork, and the neighbours this icon
# sits next to in 登录项与扩展 fill 82-100% of the canvas in *both* directions
# (measured from Slack, Telegram and Docker).
#
# The apeiron mark is wide and flat — very nearly 2:1 — so it cannot match
# that on both axes; something has to give. Fitting by width leaves it 41%
# tall and visibly punier than its neighbours, so scale by *area* instead: the
# mark covers as many pixels as a square icon of the same budget would, which
# is what the eye actually compares. It ends up full-width and about half
# height, which is simply what this logo is.
#
# The brand has no squarer variant to reach for — favicon.ico is this same
# lockup on a white plate, word and all — so cropping to one ring or setting
# the word beside the mark would be inventing a logo, not rendering one.
CANVAS, CONTENT = 1024, 824      # 824/1024 is the large-icon content box
scale = (CONTENT * CONTENT / (mark.width * mark.height)) ** 0.5
# Cap against the *canvas*, not the content box: capping at CONTENT would put
# the width limit back in charge and undo the area fit above. The margin is
# still honoured on the short axis, which is where it is visible.
MAX = CANVAS * 0.94
scale = min(scale, MAX / mark.width, MAX / mark.height)
size = (max(1, round(mark.width * scale)), max(1, round(mark.height * scale)))
mark = mark.resize(size, Image.LANCZOS)
canvas = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
canvas.paste(mark, ((CANVAS - size[0]) // 2, (CANVAS - size[1]) // 2), mark)
canvas.save(work / "icon.png")

iconset = work / "apeiron.iconset"
iconset.mkdir()
# Every size Finder, the Dock and 登录项与扩展 ask for, rendered from the
# 1024 master rather than from each other, so no size is a resize of a resize.
for px in (16, 32, 64, 128, 256, 512, 1024):
    canvas.resize((px, px), Image.LANCZOS).save(iconset / f"icon_{px}x{px}.png")
for px, name in ((32, "icon_16x16@2x"), (64, "icon_32x32@2x"), (256, "icon_128x128@2x"),
                 (512, "icon_256x256@2x"), (1024, "icon_512x512@2x")):
    canvas.resize((px, px), Image.LANCZOS).save(iconset / f"{name}.png")
PY

iconutil -c icns "$work/apeiron.iconset" -o apeiron.icns
echo "wrote $(pwd)/apeiron.icns ($(wc -c < apeiron.icns) bytes)"
