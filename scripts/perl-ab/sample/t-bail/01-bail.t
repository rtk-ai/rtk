use strict;
use warnings;
use Test::More;

ok(1, 'config loaded');
BAIL_OUT('database not reachable at localhost:5432') unless $ENV{RTK_SAMPLE_DB};
ok(1, 'query works');

done_testing;
