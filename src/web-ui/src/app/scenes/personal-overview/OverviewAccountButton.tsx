import type { RelayAccountSnapshot } from '@/infrastructure/account-identity';
import { relayAccountService } from '@/infrastructure/account-identity';
import { Avatar, Button, Icon, OverflowText } from '@openbitfun/ui';
import { LogOut } from 'lucide-react';
import { useState } from 'react';
import { useI18n } from '@/infrastructure/i18n';
import { useNotification } from '@/shared/notification-system';
import { createLogger } from '@/shared/utils/logger';

const log = createLogger('OverviewAccountButton');

/**
 * Relay-account entry for the personal overview: the masked phone with a
 * letter avatar (relay accounts have no avatar image) and a direct sign-out.
 */
export function OverviewAccountButton({ identity }: { identity: RelayAccountSnapshot }) {
  const { t } = useI18n('common');
  const notification = useNotification();
  const [signingOut, setSigningOut] = useState(false);
  if (!identity.resolved || identity.status !== 'signed-in' || !identity.me) return null;
  const username = identity.me.user.login.trim();
  const handleLogout = async () => {
    if (signingOut) return;
    setSigningOut(true);
    try {
      await relayAccountService.logout();
    } catch (error) {
      log.warn('overview account sign-out failed', error);
      notification.error(t('accountLogin.signOutFailed'));
    } finally {
      setSigningOut(false);
    }
  };
  return (
    <div className="overview-account" data-openbitfun-scene="personal-overview" data-openbitfun-part="accountActions">
      <Avatar size="sm" src={identity.me.user.avatarUrl || undefined} alt={username} aria-label={username}>
        {username.charAt(0).toUpperCase() || <Icon name="user" />}
      </Avatar>
      <OverflowText className="overview-account__name" title={username}>{username}</OverflowText>
      <Button
        variant="outline"
        size="sm"
        leadingIcon={<LogOut size={14} />}
        onClick={() => void handleLogout()}
        disabled={signingOut}
      >
        {t('accountLogin.logout')}
      </Button>
    </div>
  );
}
