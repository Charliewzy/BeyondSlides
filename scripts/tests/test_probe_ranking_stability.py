from __future__ import annotations

import json
import sys
import unittest
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import probe_ranking_stability as experiment


class RankingStabilityTests(unittest.TestCase):
    def test_schedule_matches_saved_rust_regression_groups(self):
        plan = experiment.comparison_plan(292, 8, 20260905)
        self.assertEqual(plan[0]["passage_ids"], [171, 148, 269, 151])
        self.assertEqual(plan[2]["passage_ids"], [169, 245, 124, 45])
        self.assertEqual(plan[6]["passage_ids"], [222, 188, 38, 288])
        self.assertEqual(plan[7]["passage_ids"], [228, 99, 275, 219])

    def test_each_seed_has_eight_complete_nonoverlapping_rounds(self):
        seeds = [{seed + i for i in range(8)} for seed in experiment.SEEDS]
        self.assertFalse(seeds[0] & seeds[1] or seeds[0] & seeds[2] or seeds[1] & seeds[2])
        for seed in experiment.SEEDS:
            plan = experiment.comparison_plan(292, 8, seed)
            self.assertEqual(len(plan), 584)
            counts = Counter(pid for c in plan for pid in c["passage_ids"])
            self.assertEqual(counts, Counter({pid: 8 for pid in range(292)}))
            for offset in range(0, 584, 73):
                self.assertEqual(sorted(pid for c in plan[offset:offset + 73] for pid in c["passage_ids"]), list(range(292)))

    def test_scoring_matches_rust_ties_and_levels(self):
        plan = [dict(comparison_id=0, round_index=0, passage_ids=[0, 1, 2, 3])]
        ranks = experiment.aggregate(4, plan, {"0": dict(most=0, least=3)}, 1)
        self.assertEqual([r["display_level"] for r in ranks], [5, 3, 3, 1])
        self.assertEqual([r["percentile_basis_points"] for r in ranks], [10000, 5000, 5000, 0])
        self.assertEqual([r["average_rank"] for r in ranks], [4, 2.5, 2.5, 1])
        self.assertAlmostEqual(experiment.compare_rankings(ranks, ranks)["spearman"], 1)

    def test_top_cutoff_shares_ties_without_id_tiebreak(self):
        ranking = [dict(passage_id=pid, score=score) for pid, score in enumerate([1, .5, .5, 0])]
        weights = experiment.fractional_top_k(ranking, 2)
        self.assertEqual(weights, {0: 1, 1: .5, 2: .5})
        self.assertEqual(sum(weights.values()), 2)

    def test_all_ties_have_no_defined_rank_correlation(self):
        ranking = experiment.rank_counts([dict(comparisons=8, most=2, least=2) for _ in range(4)])
        self.assertEqual([r["percentile_basis_points"] for r in ranking], [5000] * 4)
        self.assertIsNone(experiment.compare_rankings(ranking, ranking)["spearman"])

    def test_reversed_rankings_have_negative_correlation(self):
        left = experiment.rank_counts([dict(comparisons=8, most=i, least=0) for i in range(4)])
        right = experiment.rank_counts([dict(comparisons=8, most=3-i, least=0) for i in range(4)])
        result = experiment.compare_rankings(left, right)
        self.assertAlmostEqual(result["spearman"], -1)
        self.assertEqual(result["extreme_swaps"], 2)

    def test_missing_or_invalid_decisions_are_not_silently_aggregated(self):
        plan = [dict(comparison_id=0, round_index=0, passage_ids=[0, 1, 2, 3])]
        for decisions in ({}, {"0": dict(most=99, least=0)}, {"0": dict(most=1, least=1)}):
            with self.assertRaises(ValueError):
                experiment.aggregate(4, plan, decisions, 1)

    def test_requests_decode_labels_back_to_original_ids(self):
        sample = dict(passage_count=292, passages=[dict(text=f"passage-{pid}") for pid in range(292)],
                      lecture_context="test", prompt="test", model="test")
        requests, _ = experiment.request_plan(sample)
        self.assertEqual(len(requests), 111)
        for spec in requests:
            answer = dict(comparisons=[dict(comparison_id=int(cid), most="C", least="A") for cid in spec["mapping"]])
            body = dict(choices=[dict(finish_reason="stop", message=dict(content=json.dumps(answer)))])
            decoded = experiment.decode(spec, body)
            self.assertFalse(decoded["errors"])
            for cid, choice in decoded["decisions"].items():
                self.assertEqual(choice, dict(most=spec["mapping"][cid]["C"], least=spec["mapping"][cid]["A"]))


if __name__ == "__main__":
    unittest.main()
