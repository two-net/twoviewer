import { Component, ElementRef, inject, signal, viewChild } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { NavigationEnd, Router, RouterLink } from '@angular/router';
import { filter } from 'rxjs';
import { SignOut } from '../accounts/sign-out';
import { ErrorMessage } from '../shared/errors/error-message';
import { ThemePicker } from '../shared/theme/theme-picker';

/**
 * The user menu in the header of the signed-in screens: the theme option (THM-08), a link to
 * Settings, and signing out (AUTH-09). It closes on Escape, on a click outside it, and on
 * navigation.
 */
@Component({
  selector: 'app-account-menu',
  imports: [ErrorMessage, RouterLink, ThemePicker],
  templateUrl: './account-menu.html',
  styleUrl: './account-menu.scss',
  host: {
    '(document:keydown.escape)': 'close(true)',
    '(document:click)': 'closeOutside($event)',
  },
})
export class AccountMenu {
  protected readonly open = signal(false);
  protected readonly signingOut = signal(false);
  /** Why signing out failed, if it did. */
  protected readonly error = signal<unknown>(null);
  private readonly signOutService = inject(SignOut);
  private readonly host: HTMLElement = inject(ElementRef).nativeElement;
  private readonly button = viewChild.required<ElementRef<HTMLButtonElement>>('button');

  constructor() {
    inject(Router)
      .events.pipe(
        filter((event) => event instanceof NavigationEnd),
        takeUntilDestroyed(),
      )
      .subscribe(() => this.open.set(false));
  }

  protected async signOut(): Promise<void> {
    this.signingOut.set(true);
    this.error.set(null);
    try {
      await this.signOutService.signOut();
    } catch (error) {
      this.error.set(error);
    } finally {
      this.signingOut.set(false);
    }
  }

  protected close(returnFocus: boolean): void {
    if (this.open()) {
      this.open.set(false);
      if (returnFocus) {
        this.button().nativeElement.focus();
      }
    }
  }

  protected closeOutside(event: MouseEvent): void {
    if (!this.host.contains(event.target as Node | null)) {
      this.close(false);
    }
  }
}
