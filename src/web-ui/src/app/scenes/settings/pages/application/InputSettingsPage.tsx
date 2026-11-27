import { SettingsPage } from '../shared/SettingsPage';
import KeyboardShortcutsSection from './KeyboardShortcutsSection';
import VoiceSettingsSection from './VoiceSettingsSection';
import TextSelectionSettingsSection from './TextSelectionSettingsSection';
import MessageSendingSettingsSection from './MessageSendingSettingsSection';
import ScheduleSendPresetsSettingsSection from './ScheduleSendPresetsSettingsSection';

export default function InputSettingsPage() {
  return (
    <SettingsPage pageId="application.input">
      <VoiceSettingsSection />
      <MessageSendingSettingsSection />
      <ScheduleSendPresetsSettingsSection />
      <TextSelectionSettingsSection />
      <KeyboardShortcutsSection />
    </SettingsPage>
  );
}
