"""Offline patch portability and builder diagnostics regressions."""
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent

# Unmodified tmux 3.7c context at upstream line offsets. Keep padding after each
# fragment: GNU patch can treat shortened asymmetric context as a file boundary.
CONTEXT = {
    "cmd-display-menu.c": {
        54: '\t.name = "display-popup",\n\t.alias = "popup",\n\n'
            '\t.args = { "Bb:Cc:d:e:Eh:kNs:S:t:T:w:x:y:", 0, -1, NULL },\n'
            '\t.usage = "[-BCEkN] [-b border-lines] [-c target-client] "\n'
            '\t\t "[-d start-directory] [-e environment] [-h height] "\n'
            '\t\t "[-s style] [-S border-style] " CMD_TARGET_PANE_USAGE\n'
            '\t\t " [-T title] [-w width] [-x position] [-y position] "\n',
        501: "\tif (args_has(args, 'k')) {\n\t\tif (flags == -1)\n"
             '\t\t\tflags = 0;\n\t\tflags |= POPUP_CLOSEANYKEY;\n\t}\n\n'
             '\tif (modify) {\n'
             '\t\tpopup_modify(tc, title, style, border_style, lines, flags);\n',
    },
    "tmux.h": {
        3806: '#define POPUP_INTERNAL 0x4\n#define POPUP_CLOSEANYKEY 0x8\n'
              '#define POPUP_NOJOB 0x10\n'
              'typedef void (*popup_close_cb)(int, void *);\n'
              'typedef void (*popup_finish_edit_cb)(char *, size_t, void *);\n'
              'int\t\t popup_display(int, enum box_lines, struct cmdq_item *, u_int,\n',
    },
    "format.c": {
        5892: '{\n\tstruct paste_buffer\t*pb;\n\n'
              '\tif (c != NULL && c->name != NULL)\n'
              '\t\tlog_debug("%s: c=%s", __func__, c->name);\n\telse\n',
    },
    "popup.c": {
        565: '\tconst char\t\t*buf;\n\tsize_t\t\t\t len;\n'
             '\tu_int\t\t\t px, py, x;\n'
             '\tenum { NONE, LEFT, RIGHT, TOP, BOTTOM } border = NONE;\n\n'
             '\tif (pd->md != NULL) {\n',
        587: '\t\tif (m->x < pd->px ||\n'
             '\t\t    m->x > pd->px + pd->sx - 1 ||\n'
             '\t\t    m->y < pd->py ||\n'
             '\t\t    m->y > pd->py + pd->sy - 1) {\n'
             '\t\t\tif (MOUSE_BUTTONS(m->b) == MOUSE_BUTTON_3)\n'
             '\t\t\t\tgoto menu;\n\t\t\treturn (0);\n',
    },
}


class TmuxPatchTests(unittest.TestCase):
    def test_patch_applies_without_fuzz_or_offsets(self):
        with tempfile.TemporaryDirectory(prefix="aft-patch-") as directory:
            root = Path(directory)
            for name, fragments in CONTEXT.items():
                lines = ["/* fixture padding */\n"] * (max(fragments) + 30)
                for start, text in fragments.items():
                    fragment = text.splitlines(keepends=True)
                    lines[start - 1:start - 1 + len(fragment)] = fragment
                (root / name).write_text("".join(lines))
            result = subprocess.run(
                ["patch", "--batch", "--forward", "--fuzz=0", "-p1", "-i",
                 str(ROOT / "patches/tmux-3.7c-status-mouse.patch")],
                cwd=root, capture_output=True, text=True, timeout=10,
            )
            output = result.stdout + result.stderr
            self.assertEqual(result.returncode, 0, output)
            self.assertNotIn("offset", output.lower())
            self.assertNotIn("fuzz", output.lower())
            self.assertIn("flags |= POPUP_STATUSMOUSE;", (root / "cmd-display-menu.c").read_text())
            self.assertIn("#define POPUP_STATUSMOUSE 0x20", (root / "tmux.h").read_text())
            self.assertIn('"aft_popup_status_mouse", "%d", 1', (root / "format.c").read_text())
            self.assertIn("return (2);", (root / "popup.c").read_text())

    def test_builder_failure_exposes_cause_and_keeps_log(self):
        with tempfile.TemporaryDirectory(prefix="aft-build-error-") as directory:
            root = Path(directory)
            archive = root / "invalid.tar.gz"
            archive.write_bytes(b"not the pinned release")
            build = root / "build"
            result = subprocess.run(
                ["bash", str(ROOT / "scripts/build-mouse-tmux.sh"), str(build), str(archive)],
                capture_output=True, text=True, timeout=10,
            )
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertEqual(result.stdout, "")
            self.assertIn("SHA-256 mismatch", result.stderr)
            self.assertIn("SHA-256 mismatch", (build / "build.log").read_text())
            self.assertFalse((build / "tmux-3.7c").exists())


if __name__ == "__main__":
    unittest.main()
