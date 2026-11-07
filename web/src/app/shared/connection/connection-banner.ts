import { Component, inject } from '@angular/core';
import { ServerConnection } from './server-connection';

/**
 * Says that the connection to the server is lost and being retried (UI-05). It covers nothing
 * and asks nothing of the user, who can keep working.
 */
@Component({
  selector: 'app-connection-banner',
  templateUrl: './connection-banner.html',
  styleUrl: './connection-banner.scss',
})
export class ConnectionBanner {
  protected readonly connection = inject(ServerConnection);
}
