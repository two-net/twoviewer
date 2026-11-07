import { Component } from '@angular/core';
import { PRODUCT_NAME } from '../product';

/**
 * Placeholder until the text is written. Sign-up records the accepted version (AUTH-01): change
 * PRIVACY_VERSION in `versions.ts` whenever the text changes.
 */
@Component({
  selector: 'app-privacy',
  templateUrl: './privacy.html',
})
export class Privacy {
  protected readonly productName = PRODUCT_NAME;
}
