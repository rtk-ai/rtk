package Acme::RtkSample::Util;

use strict;
use warnings;

sub trim {
    my $s = shift;
    $s =~ s/^\s+|\s+$//g;
    return $s;
}

sub pairs {
    my %h = @_;
    my @out;
    foreach my $k (keys %h) {
        push @out, "$k=$h{$k}";
    }
    return @out;
}

1;
