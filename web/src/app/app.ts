import { Component } from '@angular/core';
import { RouterOutlet } from '@angular/router';
import { SiteFooter } from './layout/site-footer';
import { ConfirmDialog } from './shared/confirm/confirm-dialog';
import { ToastRegion } from './shared/toast/toast-region';

/**
 * The root: the routed page, and what every page has. That is the footer with the policy links
 * (PRIV-04), the toasts (UI-08) and the confirmation dialog (UI-03).
 */
@Component({
  imports: [RouterOutlet, SiteFooter, ToastRegion, ConfirmDialog],
  selector: 'app-root',
  styleUrl: './app.scss',
  templateUrl: './app.html',
})
export class App {
  // A plain `#main` link would leave the page: `<base href="/">` resolves it to `/#main`.
  protected skipToMain(event: Event): void {
    event.preventDefault();
    document.getElementById('main')?.focus();
  }
}
