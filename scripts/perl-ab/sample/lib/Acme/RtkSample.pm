package Acme::RtkSample;
# ABSTRACT: sample dist for rtk filter fixtures

use strict;
use warnings;

our $VERSION = '0.01';

sub new {
    my $class = shift;
    my %args = @_;
    return bless { %args }, $class;
}

# Deliberately sloppy code so perlcritic has something to say.
sub add {
    my ($self, $a, $b) = @_;
    return $a + $b;
}

sub sorted_names {
    my $self = shift;
    return sort @{ $self->{names} || [] };
}

sub read_first_line {
    my ($self, $path) = @_;
    open FH, $path or die "cannot open $path";
    my $line = <FH>;
    close FH;
    chomp $line;
    return $line;
}

sub classify {
    my ($self, $n) = @_;
    if ($n =~ /^\d+$/) {
        if ($n > 100) {
            if ($n > 1000) {
                return 'huge';
            } else {
                return 'big';
            }
        } elsif ($n > 10) {
            return 'medium';
        } else {
            return 'small';
        }
    }
    return 'nan';
}

sub mask {
    my ($self, $n) = @_;
    return $n & 0xff;
}

sub stringy {
    my $self = shift;
    my $s = eval "1 + 1";
    return "$s";
}

1;
