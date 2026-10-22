# Contributing benchmark evidence

Keep each workload reproducible from a documented command. Pin Jhol, fixture
inputs, and comparison-tool versions; record hardware, operating system, cache
state, network conditions, and failures. Validate installed output before
publishing timing summaries, and retain raw measurements with the analysis.

Do not remove inconvenient runs or generalize one machine's result to every
project. Changes to harnesses should explain what they measure and what they
leave out. Never commit credentials, private registry data, or user project
contents.
