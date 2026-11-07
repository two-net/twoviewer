import { Component } from '@angular/core';
import { PRODUCT_NAME } from '../product';

/** Placeholder until the text is written. Sign-up records the accepted version (AUTH-01). */
@Component({
  selector: 'app-terms',
  templateUrl: './terms.html',
})
export class Terms {
  protected readonly productName = PRODUCT_NAME;
}
