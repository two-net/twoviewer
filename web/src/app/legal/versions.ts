/**
 * The versions of the Terms of Service and the Privacy Notice: the date each text last changed.
 * Sign-up sends them, and the server records them as accepted (AUTH-01). Change a version
 * whenever its text changes, together with `server/src/accounts/legal.rs`, which refuses sign-ups
 * with other versions. A server test checks that both say the same.
 */
export const TERMS_VERSION = '2026-11-08';
export const PRIVACY_VERSION = '2026-11-08';
