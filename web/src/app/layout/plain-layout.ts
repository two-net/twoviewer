import { Component } from '@angular/core';
import { RouterOutlet } from '@angular/router';
import { SiteHeader } from './site-header';

/**
 * The layout of pages without the app's navigation: the visitor screens, the host page, the
 * policies and the error pages.
 */
@Component({
  selector: 'app-plain-layout',
  imports: [RouterOutlet, SiteHeader],
  templateUrl: './plain-layout.html',
})
export class PlainLayout {}
