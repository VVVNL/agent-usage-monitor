# Update-signature interop fixtures

Made with OpenSSL 3.5 exactly as `.github/workflows/release.yml` signs a release
(`openssl pkeyutl -sign -rawin`), using a **throwaway key whose private half was discarded**. They prove
that the signature the workflow produces verifies in the program (`crates/update/src/verify.rs`) and that
the documented way of extracting the public key (`openssl pkey -pubout -outform DER | tail -c 32 | base64`)
gives what the program expects. `SHA256SUMS.txt` is the real list from release v0.11.2 (CRLF line endings).
The real project key is never stored in the repository.
