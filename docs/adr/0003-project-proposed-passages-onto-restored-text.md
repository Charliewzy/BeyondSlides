# Project proposed passage boundaries onto authoritative restored text

The annotation model returns the text of each proposed lecture passage instead
of source offsets. BeyondSlides concatenates those copies, aligns them to the
owned restored transcript with a Unicode-scalar Myers diff, and projects the
proposed boundaries onto authoritative UTF-8 source ranges.

When fewer than 5% of the larger source/proposed character count changed, the
system accepts only the model's boundary intent and replaces every copied
passage with its exact authoritative source slice. A difference of 5% or more
is rejected and returned to the model for bounded repair. Accepted projection
diagnostics retain changed and compared character counts for evaluation.

This keeps model-facing passage selection natural without trusting generated
text as lecture evidence. The threshold tolerates small copying mistakes but
does not authorize paraphrase; 5% is an initial product policy and may change
after real-course evaluation.
