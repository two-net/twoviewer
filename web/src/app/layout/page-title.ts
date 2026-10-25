import { Injectable, inject } from '@angular/core';
import { Title } from '@angular/platform-browser';
import { RouterStateSnapshot, TitleStrategy } from '@angular/router';
import { PRODUCT_NAME } from '../product';

/** Names the browser tab after the page, as in "Devices – TwoViewer". */
@Injectable({ providedIn: 'root' })
export class PageTitle extends TitleStrategy {
  private readonly title = inject(Title);

  override updateTitle(snapshot: RouterStateSnapshot): void {
    const page = this.buildTitle(snapshot);
    this.title.setTitle(
      page === undefined ? PRODUCT_NAME : $localize`${page}:page: – ${PRODUCT_NAME}:product:`,
    );
  }
}
