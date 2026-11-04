#!/usr/bin/perl
# pgtimeout.pl SECS CMD...: run CMD in its own process group and kill the
# whole group when SECS pass, so a hung test binary dies with the cargo that
# started it. Exits with CMD's status, 128+N for a signal N, or 124 when the
# timer fired.
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
    kill 'KILL', -$pid;
    waitpid($pid, 0);
    exit 124;
};
alarm $secs;
waitpid($pid, 0);
alarm 0;
my $st = $?;
exit(($st & 127) ? 128 + ($st & 127) : ($st >> 8));
