use strict;
use warnings;
use Test::More;

use Acme::RtkSample;

my $obj = Acme::RtkSample->new;
is($obj->add(1, 1), 2, 'adds');
is($undeclared_total, 2, 'uses a variable that was never declared');

done_testing;
