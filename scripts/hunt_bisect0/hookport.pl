#!/usr/bin/perl
# usage: perl hookport.pl SRC/DB.RS
# The forcing hook of the snapshot schedule, ported to any commit that has the block cache's published forms:
# a Snapshot handle's scan pauses for HUNT_SCAN_AFTER_CHECK_MS between its one check of the published
# forms' position and its first load of a form. Nothing else of the engine changes.
use strict;
use warnings;
my $path = shift or die "usage: hookport.pl db.rs\n";
local $/;
open(my $in, '<', $path) or die "open $path: $!";
my $src = <$in>;
close($in);
my $n = ($src =~ s{
    ^([ \t]*)(let\ canon:\ Option<&Cached>\ =\n
    [ \t]*match\ canonical\.then\(\|\|\ self\.canonical\(pi,\ b\)\)\.flatten\(\)\ \{)
}{$1if canonical && self.isolation.get() == Isolation::Snapshot {\n$1    hunt_pause("HUNT_SCAN_AFTER_CHECK_MS");\n$1}\n$1$2}mx);
die "the load site was not found in $path\n" unless $n == 1;
$src .= <<'RS';

/// HUNT: a pause of the number of milliseconds the variable names, to hold
/// a thread at a point of a forcing schedule (a hunt branch's patch, never
/// for a merge).
fn hunt_pause(var: &str) {
    if let Some(ms) = std::env::var(var).ok().and_then(|s| s.parse::<u64>().ok()) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}
RS
open(my $out, '>', $path) or die "write $path: $!";
print $out $src;
close($out);
print "hooked $path ($n site)\n";
