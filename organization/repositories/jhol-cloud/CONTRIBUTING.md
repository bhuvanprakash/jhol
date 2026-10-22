# Contributing to Jhol Cloud

This repository is intended for optional hosted cache and control-plane
services. Keep local Jhol workflows usable without it. Every change must state
its trust boundary, tenant scope, data retained, and failure behavior.

Require tests for tenant isolation, authorization, quotas, digest validation,
and retry limits when those paths change. Never add remote task execution until
worker isolation and its threat model receive a separate review. Do not include
credentials, customer data, or live provider secrets in code or fixtures.
