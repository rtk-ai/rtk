use strict;
use warnings;
use Test::More;

use Acme::RtkSample;

my $obj = Acme::RtkSample->new(names => [qw(c a b)]);

ok(1, 'object builds');
is($obj->add(2, 2), 5, 'two plus two is five');
is_deeply(
    { list => [ $obj->sorted_names ], count => 3 },
    { list => [qw(a b d)],           count => 3 },
    'sorted names structure',
);
like($obj->classify(20), qr/^big$/, 'twenty is big');
ok(1, 'still running');

done_testing;
