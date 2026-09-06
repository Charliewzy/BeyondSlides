# Own processing jobs outside the browser

The local web application owns persistent lecture jobs and launches the existing
pipeline in a worker process, rather than running analysis inside an HTTP request
or duplicating its checkpoint logic. This isolates per-job provider settings and
credentials from process-global environment changes, preserves the CLI workflow,
and lets browser refreshes or closure leave processing untouched. Workers expose
structured progress and cooperative stopping: stop admitting new checkpointable
work, drain already-started conversations, and persist their accepted results.
The browser is a reconnectable controller and viewer, not the owner of execution.
