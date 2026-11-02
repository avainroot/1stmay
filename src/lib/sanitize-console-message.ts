const URL_PATTERN = /\b(?:https?|wss?|ftp):\/\/[^\s"'<>]+/gi;

export function sanitizeConsoleMessage(message: string): string {
  return message.replace(URL_PATTERN, "[URL скрыт]");
}
