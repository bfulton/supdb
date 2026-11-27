#!/usr/bin/env python3
"""Every commit hash the tracked tree names, and where each one resolves.

Run from the repository root, with every ref fetched (main's history whole,
and `git fetch origin '+refs/pull/*/head:refs/remotes/pull/*'`):

    python3 release/tools/citations.py            one line per hash as cited
    python3 release/tools/citations.py --sites    one line per citing site
    python3 release/tools/citations.py --abbrev   how short a hash may be cut

It reads the tracked text files of the working tree, release/ left out (the
audit's own output, which quotes this tool). A token is 7 to 40 lowercase hex
characters with no letter, digit or underscore on either side, so a `0x`
literal is never one. A token is a citation when one of these holds:

  resolves  it is the name, whole or abbreviated, of exactly one commit here.
            In a source file that counts only in a comment or in a row's file
            name (<utc>-<hash>); anywhere else in code it is a constant, left
            out and counted on stderr.
  context   the text says commit whether or not it resolves: a row's file
            name and its "sha" field; a backticked token with a hex letter;
            a token after at, commit, since, upstream, show or diff (one of
            digits alone only when it is backticked as well).
  external  another repository's: a vendor PIN file, the compiler's own hash
            in a `rustc 1.2.3 (<hash> <date>)` stamp, or the hash that ends a
            dependency's version in a lock file (`-<n>-g<hash>`).

A row (bench/runs/**.json) is read as its file name and its string fields.
Its numbers are measurements and are never read.

Classes: 1 on origin/main's first-parent line (the hashes a rewrite of main
would change), 2 reachable from a pull ref and not on main, 3 reachable only
from another origin branch, 4 a non-commit object, 5 ambiguous as written,
6 said to be a commit by its context and reachable from no remote ref (no
full hash: no such object), 7 another repository's. The pull_requests column
lists every pull ref the commit is reachable from, for class 1 as well: those
refs keep a hash readable whatever happens to main. A site's line is 0 where
the site is the file's own name.

Exit status 1 if any class 5 or 6 exists, else 0.
"""
import bisect
import collections
import os
import re
import subprocess
import sys

MAIN = 'refs/remotes/origin/main'
# The second lookbehind takes the hash of a `git describe` name, 1.2-3-g<hash>.
TOKEN = re.compile(r'(?:(?<![0-9A-Za-z_])|(?<=\d-g))[0-9a-f]{7,40}'
                   r'(?![0-9A-Za-z_])')
ROW = re.compile(r'bench/runs/.*-([0-9a-f]{7,40})\.json$')
ROW_FIELD = re.compile(r'\s*"(\w+)": "(.*)",?$')
ROW_NAME = re.compile(r'\d{8}T\d{6}Z-$')
CITING = re.compile(r'\b(at|commit|since|upstream|show|diff)\s+`?$', re.I)
RUSTC = re.compile(r'rustc [0-9.]+ \($')
PROSE = {'md', 'txt', 'svg', 'html', ''}
SLASH_COMMENTS = {'rs', 'c', 'cc', 'h', 'mjs', 'js'}
DISAGREE = []       # rows whose file name is not the start of their sha


def git(*args, sep='\n'):
    out = subprocess.run(('git',) + args, check=True, capture_output=True,
                         text=True).stdout
    return out.split(sep)[:-1]


def in_comment(ext, before):
    if ext in SLASH_COMMENTS:
        return '//' in before or re.match(r'\s*/?\*', before) is not None
    return '#' in before


def candidates(path, text):
    """Yield (line, token, how): how is 'external', 'context', 'resolves'
    (a citation only if it resolves) or 'code' (a constant if it resolves)."""
    ext = os.path.splitext(path)[1][1:]
    row = ROW.match(path)
    if row:
        yield 0, row.group(1), 'context'
        if not re.search(r'^\s*"sha": "%s' % row.group(1), text, re.M):
            DISAGREE.append(path)
    for number, line in enumerate(text.split('\n'), 1):
        key = None
        if row:
            field = ROW_FIELD.match(line) if '": "' in line else None
            if not field:
                continue
            key, line = field.groups()
        for m in TOKEN.finditer(line):
            token, before, after = m.group(), line[:m.start()], line[m.end():]
            ticked = before.endswith('`') and after.startswith('`')
            letters = not token.isdigit()
            if (os.path.basename(path) == 'PIN' or RUSTC.search(before)
                    or (ext == 'lock' and before.endswith('-g'))):
                how = 'external'
            elif key == 'sha':
                how = 'context'
            elif (not row and ext not in PROSE and not in_comment(ext, before)
                  and not ROW_NAME.search(before)):
                how = 'code'
            elif (ticked and letters) or (CITING.search(before)
                                           and (ticked or letters)):
                how = 'context'
            else:
                how = 'resolves'
            yield number, token, how


def prefixed(names, token):
    i, hits = bisect.bisect_left(names, token), []
    while i < len(names) and names[i].startswith(token):
        hits.append(names[i])
        i += 1
    return hits


def ranges(numbers):
    out = []
    for n in sorted(numbers):
        if out and out[-1][1] == n - 1:
            out[-1][1] = n
        else:
            out.append([n, n])
    return ','.join(str(a) if a == b else '%d-%d' % (a, b) for a, b in out)


def needs(names, full):
    """The shortest prefix of `full` that names it alone among `names`."""
    i = bisect.bisect_left(names, full)
    near = [names[j] for j in (i - 1, i + 1) if 0 <= j < len(names)]
    return 1 + max([len(os.path.commonprefix([full, n])) for n in near] or [0])


def main():
    kind = dict(line.split() for line in git(
        'cat-file', '--batch-all-objects',
        '--batch-check=%(objectname) %(objecttype)'))
    names = sorted(kind)
    on_main = set(git('rev-list', '--first-parent', MAIN))
    pulls = collections.defaultdict(set)
    branches = collections.defaultdict(set)
    for ref in git('for-each-ref', '--format=%(refname)', 'refs/remotes'):
        remote, name = ref.split('/', 3)[2:]
        if remote == 'pull':
            for commit in git('rev-list', ref):
                pulls[commit].add(int(name.split('/')[0]))
        elif remote == 'origin' and ref != MAIN and name != 'HEAD':
            for commit in git('rev-list', ref, '--not', MAIN):
                branches[commit].add(name)

    sites = []          # (path, line, token, class, full hash)
    constants = 0
    for path in git('ls-files', '-z', sep='\0'):
        if path.startswith('release/'):
            continue
        with open(path, 'rb') as f:
            data = f.read()
        if b'\0' in data[:8192]:
            continue
        text = data.decode('utf-8', 'replace')
        for line, token, how in candidates(path, text):
            hits = prefixed(names, token)
            commits = [h for h in hits if kind[h] == 'commit']
            full = commits[0] if len(commits) == 1 else ''
            if how == 'external':
                cls, full = 7, ''
            elif how == 'code':
                constants += len(commits) == 1
                continue
            elif len(hits) > 1 and (commits or how == 'context'):
                cls = 5
            elif full:
                cls = (1 if full in on_main else 2 if full in pulls
                       else 3 if full in branches else 6)
            elif how == 'context':
                cls = 4 if hits else 6
            else:
                continue
            sites.append((path, line, token, cls, full))

    cited = collections.defaultdict(collections.Counter)
    for path, line, token, cls, full in sites:
        if path.startswith('docs/figures/'):
            path = os.path.dirname(path) + '/*' + os.path.splitext(path)[1]
        cited[cls, full, token][path] += 1

    if '--sites' in sys.argv:
        print('file\tline\thash_as_cited\tclass')
        for path, line, token, cls, full in sites:
            print('%s\t%d\t%s\t%d' % (path, line, token, cls))
    elif '--abbrev' in sys.argv:
        commits = [n for n in names if kind[n] == 'commit']
        print('shortest length at which no two objects share a prefix: %d'
              ' (%d objects); no two commits: %d (%d commits)' % (
                  max(needs(names, n) for n in names), len(names),
                  max(needs(commits, n) for n in commits), len(commits)))
        print('citations that two characters fewer would make ambiguous:')
        print('hash_as_cited\tfull_hash\tclass\tsites'
              '\tneeds_among_objects\tneeds_among_commits')
        for (cls, full, token), files in sorted(cited.items()):
            if full and len(token) - needs(names, full) < 2:
                print('%s\t%s\t%d\t%d\t%d\t%d' % (
                    token, full, cls, sum(files.values()),
                    needs(names, full), needs(commits, full)))
    else:
        print('hash_as_cited\tfull_hash\tclass\tpull_requests\tbranches'
              '\tsites\tfiles')
        for (cls, full, token), files in sorted(cited.items()):
            print('\t'.join([
                token, full, str(cls), ranges(pulls.get(full, ())),
                ','.join(sorted(branches.get(full, ()))) if cls == 3 else '',
                str(sum(files.values())),
                ';'.join('%s:%d' % f for f in sorted(files.items()))]))

    summary = []
    for cls in range(1, 8):
        of = [(full or token[:7], sum(files.values()))
              for (c, full, token), files in cited.items() if c == cls]
        summary.append('class %d: %d hashes, %d sites' % (
            cls, len({h for h, _ in of}), sum(n for _, n in of)))
    summary.append('rows whose name and sha disagree: %d' % len(DISAGREE))
    summary.append('resolving tokens left out as constants in code: %d'
                   % constants)
    print('; '.join(summary), file=sys.stderr)
    return 1 if any(cls in (5, 6) for cls, _, _ in cited) else 0


if __name__ == '__main__':
    sys.exit(main())
