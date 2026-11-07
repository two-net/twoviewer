import { Component, ElementRef, inject, signal, viewChild } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { NavigationEnd, Router, RouterLink } from '@angular/router';
import { filter } from 'rxjs';
import { ThemePicker } from '../shared/theme/theme-picker';

/**
 * The user menu in the header of the signed-in screens: the theme option (THM-08) and a link to
 * Settings. It closes on Escape, on a click outside it, and on navigation.
 */
@Component({
  selector: 'app-account-menu',
  imports: [RouterLink, ThemePicker],
  templateUrl: './account-menu.html',
  styleUrl: './account-menu.scss',
  host: {
    '(document:keydown.escape)': 'close(true)',
    '(document:click)': 'closeOutside($event)',
  },
})
export class AccountMenu {
  protected readonly open = signal(false);
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
