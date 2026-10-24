"""Timing contract only; no audio, model loading, downloads or microphone."""
import unittest
from types import SimpleNamespace as Value
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path

spec = spec_from_file_location("transcribe", Path(__file__).with_name("transcribe.py"))
module = module_from_spec(spec)
spec.loader.exec_module(module)
timed_words = module.timed_words


class TimingContract(unittest.TestCase):
    def segment(self, start, end):
        return Value(text="Hallo", words=[Value(start=start, end=end, word="Hallo")])

    def test_words_are_clipped_to_recording_and_padding_is_omitted(self):
        result = timed_words([self.segment(0.5, 2), self.segment(2, 3)], 1)
        self.assertEqual(result, {
            "duration_seconds": 1,
            "segments": [{"start": 0.5, "end": 1, "text": "Hallo"}],
        })

    def test_invalid_or_missing_timing_is_rejected(self):
        for segments, duration in [
            ([self.segment(-1, 1)], 2),
            ([self.segment(2, 1)], 2),
            ([self.segment(1, 2), self.segment(0, 1)], 3),
            ([Value(text="Hallo", words=None)], 2),
            ([self.segment(0, float("nan"))], 2),
            ([], float("inf")),
        ]:
            with self.subTest(segments=segments, duration=duration):
                with self.assertRaises(ValueError):
                    timed_words(segments, duration)


if __name__ == "__main__":
    unittest.main()
