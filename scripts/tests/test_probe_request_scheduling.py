import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from probe_request_scheduling import Controller, simulate


class SchedulingPolicyTests(unittest.TestCase):
    def test_correlated_throttles_reduce_once_but_extend_cooldown(self):
        policy = Controller("adaptive", cap=8)
        generation = policy.admit(0, 0)
        policy.finish(1, generation, 429, 1, 3)
        policy.finish(2, generation, 429, 2, 4)
        self.assertEqual(policy.cap, 4)
        self.assertEqual(policy.cooldown, 6)
        for _ in range(20):
            policy.finish(10, generation, 200, 1)
        self.assertEqual(policy.cap, 4)

    def test_quota_at_cap_one_needs_spacing(self):
        policy = Controller("adaptive", cap=1)
        for now in range(4):
            policy.finish(now, policy.generation, 429, .1, 1)
        self.assertEqual(policy.cap, 1)
        self.assertGreaterEqual(policy.spacing, 4)

    def test_serial_successes_do_not_create_demand(self):
        policy = Controller("adaptive")
        for now in range(30):
            generation = policy.admit(now, 0)
            policy.finish(now + .5, generation, 200, .5)
        self.assertEqual(policy.cap, 2)

    def test_simulator_never_exceeds_hard_cap_and_completes(self):
        for scenario in ["unlimited", "concurrency_4", "requests_3_per_second",
                         "slow_request_quota", "capacity_drop", "transient_503"]:
            result = simulate("adaptive", scenario, 0)
            self.assertEqual(result["completed"], 160, scenario)
            self.assertEqual(result["failed"], 0, scenario)
            self.assertLessEqual(result["peak_inflight"], 8, scenario)


if __name__ == "__main__":
    unittest.main()
