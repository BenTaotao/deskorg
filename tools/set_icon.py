#!/usr/bin/env python3
"""Embed the DeskOrg app icon into a Windows exe as RT_GROUP_ICON/RT_ICON.

Windows Explorer and the taskbar read this resource for the executable/file
icon. The egui window icon is loaded separately from assets/icon.png.
"""

from __future__ import annotations

import ctypes
import struct
import sys
from ctypes import wintypes
from pathlib import Path

from PIL import Image

SIZES = (16, 24, 32, 48, 64, 128, 256)
LANG = 0x0409


def dib_for(image: Image.Image, size: int) -> bytes:
    rgba = image.convert("RGBA").resize((size, size), Image.Resampling.LANCZOS)
    pixels = rgba.load()
    xor = bytearray()
    for y in range(size - 1, -1, -1):
        for x in range(size):
            r, g, b, a = pixels[x, y]
            xor += bytes((b, g, r, a))
    # 1-bpp AND mask, rows padded to 32 bits. 32-bpp alpha icons use the
    # alpha channel; an all-zero mask keeps legacy renderers happy.
    mask_row = ((size + 31) // 32) * 4
    mask = bytes(mask_row * size)
    header = struct.pack(
        "<IiiHHIIiiII",
        40,  # biSize
        size,
        size * 2,  # XOR + AND mask
        1,
        32,
        0,
        size * size * 4,
        0,
        0,
        0,
        0,
    )
    return header + bytes(xor) + mask


def group_icon(dibs: list[bytes]) -> bytes:
    body = struct.pack("<HHH", 0, 1, len(dibs))
    for index, data in enumerate(dibs, start=1):
        size = SIZES[index - 1]
        body += struct.pack(
            "<BBBBHHIH",
            0 if size == 256 else size,
            0 if size == 256 else size,
            0,
            0,
            1,
            32,
            len(data),
            index,
        )
    return body


def update_resource(
    handle: int,
    res_type: int,
    res_name: int,
    data: bytes,
    update,
) -> None:
    buffer = ctypes.create_string_buffer(data)
    if not update(
        handle,
        ctypes.c_void_p(res_type),
        ctypes.c_void_p(res_name),
        LANG,
        ctypes.cast(buffer, ctypes.c_void_p),
        len(data),
    ):
        raise ctypes.WinError(ctypes.get_last_error())


def stamp(exe: Path, png: Path) -> None:
    image = Image.open(png)
    dibs = [dib_for(image, size) for size in SIZES]
    group = group_icon(dibs)

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    begin = kernel32.BeginUpdateResourceW
    begin.argtypes = [wintypes.LPCWSTR, wintypes.BOOL]
    begin.restype = wintypes.HANDLE
    update = kernel32.UpdateResourceW
    update.argtypes = [
        wintypes.HANDLE,
        ctypes.c_void_p,
        ctypes.c_void_p,
        wintypes.WORD,
        ctypes.c_void_p,
        wintypes.DWORD,
    ]
    update.restype = wintypes.BOOL
    end = kernel32.EndUpdateResourceW
    end.argtypes = [wintypes.HANDLE, wintypes.BOOL]
    end.restype = wintypes.BOOL

    handle = begin(str(exe), False)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        for index, data in enumerate(dibs, start=1):
            update_resource(handle, 3, index, data, update)  # RT_ICON
        update_resource(handle, 14, 1, group, update)  # RT_GROUP_ICON
    finally:
        if not end(handle, False):
            raise ctypes.WinError(ctypes.get_last_error())


def main() -> int:
    if len(sys.argv) not in (2, 3):
        print(f"usage: {Path(sys.argv[0]).name} <exe> [icon.png]", file=sys.stderr)
        return 2
    exe = Path(sys.argv[1]).resolve()
    png = Path(sys.argv[2]).resolve() if len(sys.argv) == 3 else (
        Path(__file__).resolve().parent.parent / "assets" / "icon.png"
    )
    if not exe.is_file() or not png.is_file():
        print(f"missing exe or icon: {exe} / {png}", file=sys.stderr)
        return 2
    stamp(exe, png)
    print(f"icon embedded: {exe}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
