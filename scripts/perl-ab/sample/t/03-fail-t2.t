use Test2::V0;

use Acme::RtkSample::Util;

my %got = map { split /=/ } Acme::RtkSample::Util::pairs(a => 1, b => 2);

is(\%got, { a => 1, b => 3 }, 'pairs round-trips');
is(Acme::RtkSample::Util::trim("\tx\n"), 'x', 'trim tabs and newlines');

done_testing;
