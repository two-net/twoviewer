import { Component, inject } from '@angular/core';
import { Theme } from './theme';

let pickers = 0;

/**
 * The theme option as a group of radio buttons (THM-08), in Settings → Appearance and in the
 * account menu. A choice applies at once (THM-03).
 */
@Component({
  selector: 'app-theme-picker',
  templateUrl: './theme-picker.html',
  styleUrl: './theme-picker.scss',
})
export class ThemePicker {
  protected readonly theme = inject(Theme);
  /** Each picker is its own radio group, since two can be on the page at once. */
  protected readonly name = `theme-${++pickers}`;
}
