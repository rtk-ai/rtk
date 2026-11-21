use strict;
use warnings;
use Test::More;

use Acme::RtkSample;
use Acme::RtkSample::Util;

my $obj = Acme::RtkSample->new(names => [qw(c a b)]);

subtest 'arithmetic' => sub {
    is($obj->add(1, 2), 3, 'one plus two');
    is($obj->add(0, 0), 0, 'zero plus zero');
    is($obj->add(-1, 1), 0, 'negative one plus one');
};

subtest 'classify' => sub {
    is($obj->classify(5),    'small',  'five is small');
    is($obj->classify(50),   'medium', 'fifty is medium');
    is($obj->classify(500),  'big',    'five hundred is big');
    is($obj->classify(5000), 'huge',   'five thousand is huge');
    is($obj->classify('x'),  'nan',    'letters are nan');
};

subtest 'util' => sub {
    is(Acme::RtkSample::Util::trim('  hi  '), 'hi', 'trim both sides');
    is($obj->mask(0x1ff), 0xff, 'mask keeps low byte');
};

done_testing;
