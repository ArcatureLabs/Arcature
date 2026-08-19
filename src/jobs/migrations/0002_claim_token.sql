-- 0002_claim_token.sql — per-claim fencing token.
--
-- Each claim generates a UUID token; every completion mutation fences on
-- (id, status='running', claim_token). A stale worker whose lease expired
-- (and was requeued by the sweep to another worker with a fresh token)
-- affects zero rows, so its stale completion event is suppressed.

ALTER TABLE arcature_jobs
    ADD COLUMN IF NOT EXISTS claim_token UUID;
