# 単体配布できる Windows GUI exe を生成する。
from pathlib import Path

from PyInstaller.utils.hooks import collect_dynamic_libs

project_root = Path(SPECPATH)
analysis = Analysis(
    [str(project_root / "pcb_clearance_gui.py")],
    pathex=[str(project_root)],
    binaries=collect_dynamic_libs("shapely"),
    datas=[],
    hiddenimports=["shapely._geos", "shapely._geometry_helpers"],
    excludes=["matplotlib", "scipy", "pandas", "IPython", "pytest", "numpy.f2py"],
    noarchive=False,
)
archive = PYZ(analysis.pure)
executable = EXE(
    archive,
    analysis.scripts,
    analysis.binaries,
    analysis.datas,
    [],
    name="PCB-Clearance",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    console=False,
    disable_windowed_traceback=False,
)
