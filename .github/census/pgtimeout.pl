#!/usr/bin/perl
# pgtimeout.pl SECS CMD...: run CMD in its own process group and kill the
# whole group when SECS pass, so a hung test binary dies with the cargo that
# started it. Exits with CMD's status, 128+N for a signal N, or 124 when the
# timer fired. PGTIMEOUT_HOOK (a command) runs with the group id before the
# kill.
use strict;
use warnings;

my $secs = shift @ARGV;
my $pid = fork();
die "fork: $!" unless defined $pid;
if ($pid == 0) {
    setpgrp(0, 0);
    exec @ARGV or die "exec $ARGV[0]: $!";
}
local $SIG{ALRM} = sub {
    print STDERR "KILLED: the ${secs} s kill timer fired\n";
    # PGTIMEOUT_HOOK, if set, runs first with the hung group's id, so the
    # group is sampled while it still hangs.
    system($ENV{PGTIMEOUT_HOOK}, $pid) if $ENV{PGTIMEOUT_HOOK};
    kill 'KILL', -$pid;
    waitpid($pid, 0);
    exit 124;
};
alarm $secs;
waitpid($pid, 0);
alarm 0;
my $st = $?;
exit(($st & 127) ? 128 + ($st & 127) : ($st >> 8));
