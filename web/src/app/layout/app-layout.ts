import { Component, ElementRef, inject, signal, viewChild } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { NavigationEnd, Router, RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';
import { filter } from 'rxjs';
import { SignOut } from '../accounts/sign-out';
import { ServerConnection } from '../shared/connection/server-connection';
import { AccountMenu } from './account-menu';
import { SiteHeader } from './site-header';

/**
 * The layout of the signed-in screens, with the main navigation and the account menu. In narrow
 * windows the navigation opens from a menu button, and closes on navigation or Escape.
 *
 * It opens the real-time connection to the server, which stays open from then on, until the
 * server says that this browser's sign-in has ended or that it has none: then it goes to the
 * sign-in page.
 */
@Component({
  selector: 'app-app-layout',
  imports: [AccountMenu, RouterLink, RouterLinkActive, RouterOutlet, SiteHeader],
  templateUrl: './app-layout.html',
  styleUrl: './app-layout.scss',
  host: { '(document:keydown.escape)': 'closeMenu()' },
})
export class AppLayout {
  protected readonly menuOpen = signal(false);
  private readonly menuButton = viewChild.required<ElementRef<HTMLButtonElement>>('menuButton');

  constructor() {
    const connection = inject(ServerConnection);
    const signOut = inject(SignOut);
    connection.signedOut.pipe(takeUntilDestroyed()).subscribe((reason) => {
      void signOut.ended(
        reason === 'ended'
          ? $localize`You've been signed out. Sign in again to continue.`
          : undefined,
      );
    });
    connection.start();
    inject(Router)
      .events.pipe(
        filter((event) => event instanceof NavigationEnd),
        takeUntilDestroyed(),
      )
      .subscribe(() => this.menuOpen.set(false));
  }

  protected closeMenu(): void {
    if (this.menuOpen()) {
      this.menuOpen.set(false);
      this.menuButton().nativeElement.focus();
    }
  }
}
