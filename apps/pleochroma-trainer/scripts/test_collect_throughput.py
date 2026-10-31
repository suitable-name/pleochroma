"""Collector unit tests; no CUDA, trainer, or database is started by these tests."""

import argparse
from pathlib import Path
import subprocess
import unittest
from unittest.mock import Mock, patch

import collect_throughput as collector


class CollectorTests(unittest.TestCase):
    def configuration(self, include_cpu=False):
        return argparse.Namespace(
            probe=Path("/probe"), optics=Path("/optics"), database=Path("/designs.sqlite"),
            output=Path("/result"), workers=9, include_cpu=include_cpu, warmup=1,
            iterations=2, entries="69,215,283", timeout_seconds=1800,
        )

    def test_cases_are_sequential_and_keep_the_workload_fixed(self):
        cases = list(collector.cases(self.configuration(include_cpu=True)))
        self.assertEqual([name for name, _ in cases],
                         ["pipeline-cuda", "pipeline-cpu", "projection-cuda", "optics-cuda"])
        cuda, cpu = cases[0][1], cases[1][1]
        for option in ("--batch-size", "--mini-batch", "--updates-per-round", "--seed", "--workers"):
            self.assertEqual(cuda[cuda.index(option) + 1], cpu[cpu.index(option) + 1])
        self.assertIn("--sync-every-iteration", cuda)
        self.assertEqual(cuda[cuda.index("--output") + 1], str(Path("/result/pipeline-cuda/report")))
        self.assertEqual(cases[2][1][cases[2][1].index("--entries") + 1], "69,215,283")
        self.assertEqual(cases[3][1][cases[3][1].index("--in-flight") + 1], "1,8,32,128")

    def test_cpu_pipeline_is_opt_in(self):
        self.assertNotIn("pipeline-cpu", [name for name, _ in collector.cases(self.configuration())])

    def test_linux_process_sample_handles_parentheses_in_comm(self):
        fields = ["S"] + ["0"] * 20
        fields[11], fields[12] = "123", "45"
        contents = {
            "/proc/42/stat": "42 (trainer (CUDA) worker) " + " ".join(fields),
            "/proc/42/status": "Name:\ttrainer\nVmRSS:\t1234 kB\nThreads:\t9\n",
        }
        with patch.object(collector, "text_file", side_effect=contents.get), \
                patch.object(collector.os, "sysconf", return_value=100, create=True):
            result = collector.process_sample(42)
        self.assertAlmostEqual(result["cpu_seconds"], 1.68)
        self.assertEqual(result["status"], {"VmRSS": "1234 kB", "Threads": "9"})

    def test_finished_child_is_never_signalled(self):
        child = Mock()
        child.poll.return_value = 0
        with patch.object(collector.os, "killpg", create=True) as kill:
            collector.stop_child(child)
        kill.assert_not_called()
        child.terminate.assert_not_called()
        child.kill.assert_not_called()

    def test_posix_timeout_targets_only_the_created_process_group(self):
        child = Mock(pid=12456)
        child.poll.return_value = None
        child.wait.side_effect = [subprocess.TimeoutExpired("probe", 10), 0]
        with patch.object(collector.os, "name", "posix"), \
                patch.object(collector.os, "killpg", create=True) as kill, \
                patch.object(collector.signal, "SIGKILL", 9, create=True):
            collector.stop_child(child)
        self.assertEqual([call.args[0] for call in kill.call_args_list], [12456, 12456])
        self.assertEqual(kill.call_count, 2)
        child.terminate.assert_not_called()
        child.kill.assert_not_called()


if __name__ == "__main__":
    unittest.main()
