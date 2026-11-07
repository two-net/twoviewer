/**
 * The server's default rules for accounts, so that forms can check them before sending. The
 * server's settings are the ones that apply (MNT-01), and its answers say which limit a field
 * didn't meet; these are constants until the web client gets settings from the server.
 */

/** `passwords.min_length` (AUTH-03). */
export const MIN_PASSWORD_LENGTH = 12;

/** `accounts.display_name_max_length`. */
export const DISPLAY_NAME_MAX_LENGTH = 100;

/** `accounts.verification_link_lifetime`, in hours (AUTH-02). */
export const VERIFICATION_LINK_HOURS = 24;
