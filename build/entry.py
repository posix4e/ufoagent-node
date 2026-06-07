"""PyInstaller entry point — frozen as ufoagentd.exe."""

import sys

from ufoagentd.cli import main

if __name__ == "__main__":
    sys.exit(main())
