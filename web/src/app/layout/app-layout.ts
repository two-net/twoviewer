import { Component, ElementRef, inject, signal, viewChild } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { NavigationEnd, Router, RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';
import { filter } from 'rxjs';
import { SiteHeader } from './site-header';

/**
 * The layout of the signed-in screens, with the main navigation. In narrow windows the
 * navigation opens from a menu button, and closes on navigation or Escape.
 */
@Component({
  selector: 'app-app-layout',
  imports: [RouterLink, RouterLinkActive, RouterOutlet, SiteHeader],
  templateUrl: './app-layout.html',
  styleUrl: './app-layout.scss',
  host: { '(document:keydown.escape)': 'closeMenu()' },
})
export class AppLayout {
  protected readonly menuOpen = signal(false);
  private readonly menuButton = viewChild.required<ElementRef<HTMLButtonElement>>('menuButton');

  constructor() {
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
