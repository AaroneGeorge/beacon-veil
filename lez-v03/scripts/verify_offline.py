#!/usr/bin/env python3
"""Offline re-check of evidence/public-testnet (no network): runs verify_testnet.py --offline.

    python3 -I lez-v03/scripts/verify_offline.py evidence/public-testnet
"""

import os
import subprocess
import sys

here = os.path.dirname(os.path.abspath(__file__))
ev = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "..", "..", "evidence", "public-testnet")
sys.exit(subprocess.call([sys.executable, "-I", os.path.join(here, "verify_testnet.py"), "--offline", ev]))
