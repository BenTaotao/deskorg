#!/usr/bin/env python3
"""Stamp the Windows VERSIONINFO resource with the publisher metadata.

This only writes the standard file-properties metadata (company, product,
version, copyright).  It is not a digital signature: Windows SmartScreen will
still say "unknown publisher" unless the exe is Authenticode-signed with a
trusted code-signing certificate.
"""

from __future__ import annotations

import ctypes
import struct
import sys
from ctypes import wintypes
from pathlib import Path

COMPANY = "白起"
PRODUCT = "DeskOrg 桌面整理与美化"
DESCRIPTION = "DeskOrg 桌面整理、桌面框框与美化工具"
VERSION = "1.1.0"
VERSION_NUM = (1, 1, 0, 0)


def align4(data: bytes) -> bytes:
    return data + b"\0" * ((4 - len(data) % 4) % 4)


def node(key: str, value: bytes, children: list[bytes], wtype: int, value_len: int) -> bytes:
    key_bytes = (key + "\0").encode("utf-16le")
    body = struct.pack("<HHH", 0, value_len, wtype) + key_bytes
    body = align4(body)
    if value:
        body += value
    body = align4(body)
    for child in children:
        body += child
    body = align4(body)
    return struct.pack("<H", len(body)) + body[2:]


def string_node(key: str, value: str) -> bytes:
    encoded = (value + "\0").encode("utf-16le")
    return node(key, encoded, [], 1, len(value) + 1)


def version_blob() -> bytes:
    major, minor, patch, build = VERSION_NUM
    file_ms = (major << 16) | minor
    file_ls = (patch << 16) | build
    fixed = struct.pack(
        "<13I",
        0xFEEF04BD,  # VS_FFI_SIGNATURE
        0x00010000,  # VS_FFI_STRUCVERSION
        file_ms,
        file_ls,
        file_ms,
        file_ls,
        0x3F,  # VS_FFI_FILEFLAGSMASK
        0,  # VS_FFI_FILEFLAGS
        0x00040004,  # VOS_NT_WINDOWS32
        1,  # VFT_APP
        0,
        0,
        0,
    )

    strings = [
        string_node("CompanyName", COMPANY),
        string_node("FileDescription", DESCRIPTION),
        string_node("FileVersion", f"{major}.{minor}.{patch}.{build}"),
        string_node("InternalName", "DeskOrg"),
        string_node("LegalCopyright", f"Copyright © 2026 {COMPANY}"),
        string_node("OriginalFilename", "DeskOrg.exe"),
        string_node("ProductName", PRODUCT),
        string_node("ProductVersion", VERSION),
        string_node("Comments", DESCRIPTION),
    ]
    string_table = node("040904B0", b"", strings, 1, 0)
    string_file_info = node("StringFileInfo", b"", [string_table], 1, 0)
    translation = struct.pack("<I", 0x040904B0)
    var = node("Translation", translation, [], 0, 4)
    var_file_info = node("VarFileInfo", b"", [var], 1, 0)

    return node(
        "VS_VERSION_INFO",
        fixed,
        [string_file_info, var_file_info],
        0,
        len(fixed),
    )


def stamp(path: Path) -> None:
    blob = version_blob()
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

    handle = begin(str(path), False)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())

    data = ctypes.create_string_buffer(blob)
    try:
        ok = update(
            handle,
            ctypes.c_void_p(16),  # RT_VERSION
            ctypes.c_void_p(1),  # VS_VERSION_INFO name
            0x0409,  # en-US (strings remain Unicode)
            ctypes.cast(data, ctypes.c_void_p),
            len(blob),
        )
        if not ok:
            raise ctypes.WinError(ctypes.get_last_error())
    finally:
        if not end(handle, False):
            raise ctypes.WinError(ctypes.get_last_error())


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {Path(sys.argv[0]).name} <exe>", file=sys.stderr)
        return 2
    path = Path(sys.argv[1]).resolve()
    if not path.is_file():
        print(f"not a file: {path}", file=sys.stderr)
        return 2
    stamp(path)
    print(f"stamped {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

