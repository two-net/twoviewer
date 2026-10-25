import { Component } from '@angular/core';
import { RouterLink } from '@angular/router';

/** The footer of every page, with the Terms of Service and Privacy Notice (PRIV-04). */
@Component({
  selector: 'app-site-footer',
  imports: [RouterLink],
  templateUrl: './site-footer.html',
  styleUrl: './site-footer.scss',
})
export class SiteFooter {}
