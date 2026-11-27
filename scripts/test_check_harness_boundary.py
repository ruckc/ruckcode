import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from check_harness_boundary import violations  # noqa: E402


def pkg(name, *deps):
    return {"name": name, "dependencies": [{"name": n, "kind": k} for n, k in deps]}


class BoundaryTests(unittest.TestCase):
    def test_clean_graph(self):
        meta = {
            "packages": [
                pkg("ruck-harness-core", ("ruck-harness-ident", None), ("serde", None)),
                pkg("ruckcode-agent", ("ruck-harness-core", None)),
            ]
        }
        self.assertEqual(violations(meta), [])

    def test_each_dependency_kind_is_caught(self):
        meta = {
            "packages": [
                pkg(
                    "ruck-harness-tools",
                    ("ruckcode-tools", None),
                    ("ruckcode-mise", "build"),
                    ("ruckcode-agent", "dev"),
                )
            ]
        }
        self.assertEqual(
            violations(meta),
            [
                ("ruck-harness-tools", "ruckcode-agent", "dev"),
                ("ruck-harness-tools", "ruckcode-mise", "build"),
                ("ruck-harness-tools", "ruckcode-tools", "normal"),
            ],
        )

    def test_product_depending_on_harness_is_fine(self):
        meta = {"packages": [pkg("ruckcode-cli", ("ruck-harness-acp", None), ("ruckcode-agent", None))]}
        self.assertEqual(violations(meta), [])


if __name__ == "__main__":
    unittest.main()
