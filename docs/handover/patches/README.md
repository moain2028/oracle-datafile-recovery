# Owner-only patches

The agent token has no `workflows` scope, so changes under `.github/workflows/` must be applied by the owner.

## release-archive-contents (ADR-0007)
Ships only what an external evaluator needs: binaries, `LICENSE`, `README_TRIAL.md` (as `README.md`), `QUICKSTART_ar.md`, `TRIAL_KIT_ar.md`, `try_odr.ps1`, `try_odr.sh`. **Stops shipping** the internal `README.md` and `CHANGELOG.md`.

Apply (from the repo root, on a branch):
```bash
git apply docs/handover/patches/release-archive-contents.patch
# or simply:
cp docs/handover/patches/release-archive-contents.yml .github/workflows/release.yml
git commit -am "ci(release): ship evaluator files only (ADR-0007)" && git push
```
Then tag `v0.3.0-alpha` to publish a build containing LV-071 (compression) and LV-072.
