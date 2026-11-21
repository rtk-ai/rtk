use strict;
use warnings;
use Test::More tests => 3;

use Acme::RtkSample;

my $obj = Acme::RtkSample->new;
ok($obj, 'constructed');
my $line = $obj->read_first_line('/nonexistent/rtk-sample.txt');
is($line, 'hello', 'first line');
ok(1, 'never reached');
