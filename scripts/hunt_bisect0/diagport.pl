#!/usr/bin/perl
# usage: perl diagport.pl SRC/DB.RS
# The stale-cut diagnostic, ported to a commit that has Reader::rebase_tables (e607e70 and after): at each rebase
# of a rewritten partition (HUNT_CHECK_CUTS=1) it counts the filed keys the table carries and how many of them have
# a cut that is not the new partition's. Nothing else of the engine changes.
use strict;
use warnings;
my $path = shift or die "usage: diagport.pl db.rs\n";
local $/;
open(my $in, '<', $path) or die "open $path: $!";
my $src = <$in>;
close($in);
my $n = ($src =~ s{
    (^[ \t]*t\.pieces\ =\ pieces;\n
     [ \t]*t\.piece_ranks\ =\ piece_ranks;\n
     ([ \t]*)if\ rebased\ \{\n)
}{$1$2    if keep_snap && std::env::var("HUNT_CHECK_CUTS").is_ok() {
$2        let mut stale = 0usize;
$2        let mut total = 0usize;
$2        for (b, list) in t.added.iter().enumerate() {
$2            for &(slot, cut) in list {
$2                if slot as usize >= self.mem().len() {
$2                    continue;
$2                }
$2                total += 1;
$2                let key = self.mem().key_of(self.mem().entry(slot as usize));
$2                let (nb, ncut) = BuildCtx::owner_of(seg, key);
$2                if nb != b || ncut != cut {
$2                    stale += 1;
$2                }
$2            }
$2        }
$2        eprintln!(
$2            "HUNTCUTS rebase p {p}: {} filed keys carried, {} with a cut that is not the new partition's (keep_snap {keep_snap})",
$2            total, stale
$2        );
$2    }
}mx);
die "the rebase site was not found exactly once in $path ($n)\n" unless $n == 1;
open(my $out, '>', $path) or die "write $path: $!";
print $out $src;
close($out);
print "diag ported to $path\n";
