from __future__ import annotations

import contextlib
import io
import sys
import unittest
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import compare_retrieval_visual as evaluation


class SemanticAlignmentMetricsTests(unittest.TestCase):
    def test_slide_distance_uses_presentation_order_instead_of_slide_ids(self) -> None:
        comparisons = [
            evaluation.WindowComparison(
                number=1,
                start_ms=0,
                end_ms=1_000,
                frame_count=1,
                dominant_slide=30,
                dominant_share=1.0,
                observed_slides=(30,),
                observed_slide_counts=((30, 1),),
                candidate_slides=(30,),
                dominant_rank=1,
            )
        ]
        semantic_alignment = {
            "windows": [
                {
                    "number": 1,
                    "slide_position": 20,
                    "slide_scores": [
                        {"slide_id": 10, "score": 0.1},
                        {"slide_id": 20, "score": 0.8},
                        {"slide_id": 30, "score": 0.7},
                    ],
                }
            ]
        }

        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            evaluation.print_semantic_alignment_metrics(
                comparisons, semantic_alignment
            )

        within_one = next(
            line for line in output.getvalue().splitlines() if line.startswith("within 1")
        )
        self.assertEqual(within_one.split()[2:], ["100.0%", "100.0%", "100.0%"])


if __name__ == "__main__":
    unittest.main()
