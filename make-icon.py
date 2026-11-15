#!/usr/bin/env python3
"""Generate the VRDoctor icon: dark rounded square, teal VR glyph, red cross."""
from PIL import Image, ImageDraw, ImageFont

S = 512
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
d.rounded_rectangle([16, 16, S - 16, S - 16], radius=96, fill=(24, 28, 34, 255))

font_path = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
try:
    font = ImageFont.truetype(font_path, 190)
    small = ImageFont.truetype(font_path, 64)
except OSError:
    font = ImageFont.load_default()
    small = font

d.text((S / 2, 215), "VR", font=font, fill=(64, 224, 208, 255), anchor="mm")
# Red cross (the "doctor" part), top-right.
cx, cy, arm, thick = 396, 116, 44, 16
d.rectangle([cx - thick, cy - arm, cx + thick, cy + arm], fill=(235, 80, 80, 255))
d.rectangle([cx - arm, cy - thick, cx + arm, cy + thick], fill=(235, 80, 80, 255))
d.text((S / 2, 400), "DOCTOR", font=small, fill=(230, 236, 240, 255), anchor="mm")

img.save("vrdoctor.png")
print("wrote vrdoctor.png")
