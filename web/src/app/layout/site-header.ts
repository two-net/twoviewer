import { Component } from '@angular/core';
import { RouterLink } from '@angular/router';
import { PRODUCT_NAME } from '../product';

/** The header of every layout: the logo and product name (UI-01), then whatever the layout adds. */
@Component({
  selector: 'app-site-header',
  imports: [RouterLink],
  templateUrl: './site-header.html',
  styleUrl: './site-header.scss',
})
export class SiteHeader {
  protected readonly productName = PRODUCT_NAME;
}
