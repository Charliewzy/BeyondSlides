# Preserve absent transcript timing instead of inventing it

Uploaded text can support lecture analysis without supporting recording playback,
so transcript-segment start/end timestamps are both optional; a validated
transcript has complete timing everywhere or none anywhere. Unknown timing is
serialized as null, never substituted with zero or estimated from text length.
Untimed windowing uses its character budget alone, and the reader labels missing
timing and does not enable passage audio without independently aligned playback
intervals. Existing numeric-timestamp JSON and model inputs serialize unchanged,
so this addition does not invalidate existing timed-source checkpoints.
