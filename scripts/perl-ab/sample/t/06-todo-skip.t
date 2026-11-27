use strict;
use warnings;
use Test::More;

use Acme::RtkSample;

my $obj = Acme::RtkSample->new;

TODO: {
    local $TODO = 'rounding not implemented';
    is($obj->add(0.1, 0.2), 0.3, 'floating point add');
}

SKIP: {
    skip 'no network in CI', 2 unless $ENV{RTK_SAMPLE_NETWORK};
    ok(0, 'fetch remote');
    ok(0, 'parse remote');
}

ok(1, 'plain pass');

done_testing;
