import { Component, inject } from '@angular/core';
import { RELOAD } from './reload';

/**
 * Shown when a page couldn't be loaded because the browser is offline. It loads the page as soon
 * as the browser is back online. It has no *Retry*, which would only show the browser's own
 * offline page.
 */
@Component({
  selector: 'app-offline',
  templateUrl: './offline.html',
  host: { '(window:online)': 'reload()' },
})
export class Offline {
  protected readonly reload = inject(RELOAD);
}
