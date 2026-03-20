# Security Policy

## Supported versions

| Version | Supported |
|---------|-----------|
| `main` branch (pre-release) | ✅ |

Once the first release is published, this table will list supported
release versions.

## Reporting a vulnerability

If you discover a security issue, **do not open a public GitHub issue**.

Instead, use one of these channels:

1. **GitHub Security Advisories** (preferred):
   <https://github.com/k-forss/remerge/security/advisories/new>

2. **Email**: security@forss.cc
   - Encrypt with the PGP fingerprint below if the issue is sensitive.

### What to include

- "remerge" in the subject if using email
- Description of the vulnerability
- Steps to reproduce (minimal example preferred)
- Impact assessment (what can an attacker do?)
- Suggested fix, if any

### Response timeline

- **Acknowledgement**: within 48 hours
- **Assessment**: within 7 days
- **Fix + disclosure**: coordinated with reporter, typically within 30 days

### PGP fingerprint

If you need to encrypt your report (security contact key — **not** the
release signing key):

```
45D4 3871 F014 FFF2 9D82  3C76 3810 BA93 74FD 5E67
```

Fetch: `gpg --keyserver keys.openpgp.org --recv-keys 45D43871F014FFF29D823C763810BA9374FD5E67`

## Scope

The following are in scope:

- Authentication bypass (mTLS cert verification, client-ID spoofing)
- Workorder injection or manipulation
- Privilege escalation (follower acting as main)
- Container escape or Docker socket abuse
- Build output tampering (binpkg integrity, GPG signature bypass)
- Release artifact tampering (PGP signature or attestation bypass)
- Information leakage through API responses

Out of scope:

- Denial of service via malformed requests (low severity for a build service)
- Issues in upstream dependencies (report those upstream, but let us know)
- Vulnerabilities in the reverse proxy configuration (that's your deployment)
