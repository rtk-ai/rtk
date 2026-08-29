"""Deterministic sample generator for the regex subset gitleaks rules use.

Produces a string that MATCHES a given pattern, filling character classes by
cycling through the class rather than repeating one character -- repeated
characters would score ~0 Shannon entropy and fail the rules that carry an
`entropy` floor.

Supported: literals, escapes, [...] classes (ranges + negation), (?:...) groups
with |, {n}, {n,m}, ?, *, +, \\b, \\d, \\w, \\s, ^, $.
"""
import re
import string

# Control-character escapes. Without these, `[\t\n\r]` would contribute the
# letters t, n and r to the class -- which look like ordinary token characters
# and silently corrupt generated samples.
ESCAPE_CHARS = {
    't': '\t', 'n': '\n', 'r': '\r', 'f': '\f', 'v': '\v', 'a': '\a', '0': '\0',
}

CLASS_SHORTHAND = {
    'd': string.digits,
    'w': string.ascii_letters + string.digits + '_',
    's': ' ',
    'D': string.ascii_letters,
    'W': ' ',
    'S': string.ascii_letters,
}
# Preferred fill order: mixed case + digits maximises entropy per char.
FILL = string.ascii_lowercase + string.digits + string.ascii_uppercase


# How far into a `{lo,hi}` range to fill. A fixture generated only at `lo`
# never exercises the top of a bound, so an over-narrow upper limit reads as
# passing. `UNBOUNDED_EXTRA` is how far past `lo` an open `{n,}` is taken.
LENGTH_MIN, LENGTH_MID, LENGTH_MAX = "min", "mid", "max"
UNBOUNDED_EXTRA = {LENGTH_MIN: 0, LENGTH_MID: 16, LENGTH_MAX: 48}


class Sampler:
    def __init__(self, pattern: str, length: str = LENGTH_MIN):
        self.p = pattern
        self.i = 0
        self.cursor = 0  # advances across the whole sample -> varied output
        self.length = length

    # -- helpers ---------------------------------------------------------
    def peek(self):
        return self.p[self.i] if self.i < len(self.p) else None

    def take_charset(self):
        """Parse a [...] class, return the list of allowed characters."""
        assert self.p[self.i] == '['
        self.i += 1
        negate = False
        if self.peek() == '^':
            negate = True
            self.i += 1
        chars = []
        first = True
        while self.i < len(self.p) and (self.p[self.i] != ']' or first):
            first = False
            c = self.p[self.i]
            if c == '\\':
                self.i += 1
                e = self.p[self.i]
                if e in CLASS_SHORTHAND:
                    chars.extend(CLASS_SHORTHAND[e])
                elif e in ESCAPE_CHARS:
                    chars.append(ESCAPE_CHARS[e])
                elif e == 'x':
                    chars.append(chr(int(self.p[self.i + 1:self.i + 3], 16)))
                    self.i += 2
                else:
                    chars.append(e)
                self.i += 1
                continue
            if (self.i + 2 < len(self.p) and self.p[self.i + 1] == '-'
                    and self.p[self.i + 2] != ']'):
                lo, hi = c, self.p[self.i + 2]
                chars.extend(chr(x) for x in range(ord(lo), ord(hi) + 1))
                self.i += 3
                continue
            chars.append(c)
            self.i += 1
        self.i += 1  # closing ]
        if negate:
            # Pick from a wide printable pool so `[^\w-]` yields punctuation
            # (a letter would still be excluded and break the match).
            pool = (string.ascii_letters + string.digits +
                    " .,:;/@#%&*+=?!()[]{}<>|'\"\\~^$")
            allowed = [c for c in pool if c not in set(chars)]
            chars = allowed or ['.']
        return chars

    def _pick(self, lo, hi, exact=False):
        """Repeat count for `{lo,hi}` at this sampler's length setting.

        `{n}` is a single point and ignores the setting. An open `{n,}` has no
        top to aim at, so it is taken a fixed distance past `lo` -- enough to
        prove the repeat accepts more than the minimum without generating
        megabytes.
        """
        if exact or hi == lo:
            return lo
        if self.length == LENGTH_MIN:
            return lo
        if hi is None:
            return lo + UNBOUNDED_EXTRA[self.length]
        return hi if self.length == LENGTH_MAX else lo + (hi - lo) // 2

    def take_quantifier(self):
        """Return (min_repeat, is_present) for a trailing quantifier."""
        c = self.peek()
        rep = 1
        if c == '{':
            j = self.p.index('}', self.i)
            body = self.p[self.i + 1:j]
            self.i = j + 1
            parts = body.split(',')
            lo = int(parts[0]) if parts[0] else 0
            hi = int(parts[1]) if len(parts) > 1 and parts[1] else None
            rep = self._pick(lo, hi, exact=len(parts) == 1)
            # A {0,n} span still needs content when the rule sets an entropy
            # floor, so take one element rather than none.
            if rep == 0 and (hi is None or hi >= 1):
                rep = 1
        elif c == '?':
            self.i += 1
            rep = 0
        elif c == '*':
            self.i += 1
            rep = 0
        elif c == '+':
            self.i += 1
            rep = 1
        else:
            return 1
        # Swallow a lazy/possessive marker: {n,m}?  *?  +?
        if self.peek() in ('?', '+'):
            self.i += 1
        return rep

    def fill(self, chars, count):
        out = []
        for _ in range(count):
            out.append(chars[self.cursor % len(chars)])
            self.cursor += 1
        return ''.join(out)

    # -- main ------------------------------------------------------------
    def parse(self, stop_at_paren=False):
        """Parse one alternation branch; return the generated string."""
        branches = [[]]
        while self.i < len(self.p):
            c = self.p[self.i]
            if c == ')' and stop_at_paren:
                break
            if c == '|':
                self.i += 1
                branches.append([])
                continue
            if c == '(':
                self.i += 1
                if self.peek() == '?':
                    rest = self.p[self.i:]
                    m_named = re.match(r'\?P?<[^>]*>', rest)
                    m_scoped = re.match(r'\?[a-zA-Z-]*:', rest)
                    m_flags = re.match(r'\?[a-zA-Z-]+\)', rest)
                    if m_named:                 # (?P<name>  /  (?<name>
                        self.i += m_named.end()
                    elif m_scoped:              # (?:  (?i:  (?-i:  (?ms:
                        self.i += m_scoped.end()
                    elif m_flags:               # (?i)  (?s)  -- flag only
                        self.i += m_flags.end()
                        continue
                inner = self.parse(stop_at_paren=True)
                if self.peek() == ')':
                    self.i += 1
                rep = self.take_quantifier()
                branches[-1].append(inner * max(rep, 1) if rep else '')
                continue
            if c == '[':
                chars = self.take_charset()
                rep = self.take_quantifier()
                branches[-1].append(self.fill(chars, rep))
                continue
            if c == '\\':
                self.i += 1
                e = self.p[self.i]
                self.i += 1
                if e in 'bBAzZ':      # zero-width assertions emit nothing
                    continue
                if e in CLASS_SHORTHAND:
                    rep = self.take_quantifier()
                    branches[-1].append(self.fill(CLASS_SHORTHAND[e], rep))
                    continue
                if e == 'x':
                    lit = chr(int(self.p[self.i:self.i + 2], 16))
                    self.i += 2
                else:
                    lit = e
                rep = self.take_quantifier()
                branches[-1].append(lit * rep)
                continue
            if c in '^$':
                self.i += 1
                continue
            self.i += 1
            rep = self.take_quantifier()
            if c == '.':          # wildcard: fill, don't emit a literal dot
                branches[-1].append(self.fill(FILL, rep))
            else:
                branches[-1].append(c * rep)
        # Prefer the FIRST non-empty branch for determinism.
        for b in branches:
            s = ''.join(b)
            if s:
                return s
        return ''


def sample(pattern: str, length: str = LENGTH_MIN) -> str:
    return Sampler(pattern, length).parse()


if __name__ == '__main__':
    import re
    tests = [
        r'\b(sk-ant-api03-[a-zA-Z0-9_\-]{93}AA)(?:[\x60\'"\s;]|\\[nr]|$)',
        r'\b((?:A3T[A-Z0-9]|AKIA|ASIA|ABIA|ACCA)[A-Z2-7]{16})\b',
        r'ghp_[0-9a-zA-Z]{36}',
        r'\b(AIza[\w-]{35})(?:[\x60\'"\s;]|\\[nr]|$)',
        r'(?i)\b(npm_[a-z0-9]{36})(?:[\x60\'"\s;]|\\[nr]|$)',
    ]
    for t in tests:
        s = sample(t)
        ok = bool(re.search(t, s))
        print(f"{'OK ' if ok else 'FAIL'} {s[:70]:<72}")
