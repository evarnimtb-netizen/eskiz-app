#!/usr/bin/env python3
"""Host test for ui_find.py (run: python3 -I android/tools/test_ui_find.py)."""
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))

DUMP = """<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>
<hierarchy rotation="0">
  <node text="" resource-id="com.android.documentsui:id/toolbar" content-desc="" package="com.android.documentsui" bounds="[0,100][1080,220]">
    <node text="Downloads" resource-id="android:id/title" content-desc="" package="com.android.documentsui" bounds="[100,120][300,200]" />
  </node>
  <node text="fixture.procreate" resource-id="android:id/title" content-desc="" package="com.android.documentsui" bounds="[200,400][600,480]" />
  <node text="Eskiz Share Test" resource-id="" content-desc="" package="android" bounds="[0,1500][1080,1700]" />
  <node text="" resource-id="" content-desc="Show roots" package="com.android.documentsui" bounds="[0,0][100,100]" />
  <node text="Hidden" resource-id="" content-desc="" package="android" bounds="[0,0][0,0]" />
</hierarchy>
UI hierchary dumped to: /dev/tty
"""


def run(*args):
    d = tempfile.mkdtemp()
    p = os.path.join(d, "dump.xml")
    open(p, "w").write(DUMP)
    r = subprocess.run([sys.executable, "-I", os.path.join(HERE, "ui_find.py"), p, *args], capture_output=True, text=True)
    return r.returncode, r.stdout.strip()


class UiFind(unittest.TestCase):
    def test_by_text(self):
        self.assertEqual(run("--text", "fixture.procreate"), (0, "400 440"))

    def test_by_id_with_and_without_prefix(self):
        self.assertEqual(run("--id", "com.android.documentsui:id/toolbar"), (0, "540 160"))
        self.assertEqual(run("--id", "toolbar"), (0, "540 160"))

    def test_by_desc_and_contains(self):
        self.assertEqual(run("--desc", "Show roots"), (0, "50 50"))
        self.assertEqual(run("--text-contains", "share test"), (0, "540 1600"))

    def test_index_and_package(self):
        self.assertEqual(run("--id", "title", "--index", "1"), (0, "400 440"))
        self.assertEqual(run("--id", "title", "--package", "com.android.documentsui", "--index", "0"), (0, "200 160"))

    def test_missing_and_empty_bounds(self):
        self.assertEqual(run("--text", "nothing")[0], 1)
        self.assertEqual(run("--text", "Hidden")[0], 1, "a node with no area cannot be tapped")


if __name__ == "__main__":
    unittest.main()
