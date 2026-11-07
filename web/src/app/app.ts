import { Component, inject } from '@angular/core';
import { RouterOutlet } from '@angular/router';
import { SiteFooter } from './layout/site-footer';
import { ConnectionBanner } from './shared/connection/connection-banner';
import { ConfirmDialog } from './shared/confirm/confirm-dialog';
import { Motion } from './shared/motion/motion';
import { ReauthenticateDialog } from './shared/reauthentication/reauthenticate-dialog';
import { Theme } from './shared/theme/theme';
import { ToastRegion } from './shared/toast/toast-region';

/**
 * The root: the routed page, and what every page has. That is the banner for a lost connection to
 * the server (UI-05), the footer with the policy links (PRIV-04), the toasts (UI-08), the
 * confirmation dialog (UI-03) and the dialog that asks to re-authenticate (AUTH-10).
 *
 * It starts {@link Theme} and {@link Motion} on every page, so that options chosen in another tab
 * apply here too.
 */
@Component({
  imports: [
    RouterOutlet,
    ConnectionBanner,
    SiteFooter,
    ToastRegion,
    ConfirmDialog,
    ReauthenticateDialog,
  ],
  selector: 'app-root',
  styleUrl: './app.scss',
  templateUrl: './app.html',
})
export class App {
  constructor() {
    inject(Theme);
    inject(Motion);
  }

  // A plain `#main` link would leave the page: `<base href="/">` resolves it to `/#main`.
  protected skipToMain(event: Event): void {
    event.preventDefault();
    document.getElementById('main')?.focus();
  }
}
