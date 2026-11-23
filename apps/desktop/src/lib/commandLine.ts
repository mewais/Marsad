/**
 * A command's argument list as one line to read and paste into a POSIX shell: each argument
 * that is not plain is wrapped in single quotes. Display only; the daemon's `argv` is what the
 * agent's program is given, never rebuilt from this line.
 */
const PLAIN = /^[A-Za-z0-9_@%+=:,./-]+$/;

export function quoteArg(arg: string): string {
  if (arg !== "" && PLAIN.test(arg)) return arg;
  return `'${arg.replace(/'/g, `'\\''`)}'`;
}

export function commandLine(argv: readonly string[]): string {
  return argv.map(quoteArg).join(" ");
}
