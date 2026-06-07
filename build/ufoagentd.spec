# PyInstaller spec — builds a single-file ufoagentd.exe.
# Build:  pyinstaller build/ufoagentd.spec --noconfirm
# (run from the repo root so pathex '.' resolves to the package)

block_cipher = None

a = Analysis(
    ["entry.py"],
    pathex=["."],
    binaries=[],
    datas=[],
    hiddenimports=["win32crypt"],  # optional DPAPI; ignored if absent at build time
    hookspath=[],
    runtime_hooks=[],
    excludes=[],
    cipher=block_cipher,
)
pyz = PYZ(a.pure, a.zipped_data, cipher=block_cipher)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name="ufoagentd",
    console=True,
    icon=None,
    upx=False,
)
