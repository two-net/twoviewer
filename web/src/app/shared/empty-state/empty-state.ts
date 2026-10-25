import { Component } from '@angular/core';

/**
 * What a screen shows when it has nothing to list. The content says what the next step is, and
 * elements marked `actions` take the user there (UI-04):
 *
 * ```html
 * <app-empty-state>
 *   <p i18n>No devices yet. Add your first device.</p>
 *   <a actions class="button button-primary" routerLink="/devices/add" i18n>Add device</a>
 * </app-empty-state>
 * ```
 */
@Component({
  selector: 'app-empty-state',
  templateUrl: './empty-state.html',
  styleUrl: './empty-state.scss',
})
export class EmptyState {}
