/**
 * Market account entry point.
 *
 * The market account surface is disabled: browsing stays anonymous and there
 * is no sign-in affordance (the retired GitHub OAuth flow has no relay-local
 * replacement on market surfaces). This component intentionally renders
 * nothing so existing call sites keep compiling while the product decision
 * stays centralized here. See `useMarketAccountIdentity` for the matching
 * permanently signed-out identity view.
 */

/** Retained so existing call sites keep compiling; every prop is ignored. */
export interface AccountIdentityControlsProps {
  className?: string;
  showSignIn?: boolean;
  loginOpen?: boolean;
  onLoginOpenChange?: (open: boolean) => void;
  onIdentityChanged?: () => void | Promise<void>;
}

export function AccountIdentityControls(_props: AccountIdentityControlsProps = {}) {
  return null;
}
