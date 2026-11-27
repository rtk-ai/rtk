# Questions for the Perl A/B check

Answer each question for each scenario from the output file alone. Do not read the sample
source, and do not run anything. If the output does not contain the answer, write
"not in output". Give file names, line numbers and values exactly as the output shows them.

## prove_fail, prove_fail_jobs, dzil_test, yath_fail

1. Which test files failed, and which passed?
2. For each failed assertion: the test file, the line, the test name, and what was expected
   versus what was received.
3. Which test files died or never ran their plan, and what error or message caused it?
   If the output shows the source file and line of the error, give them.
4. What is the total number of test files and assertions, and the overall result?
5. For each failure, what is the most likely fix?

## prove_bailout

1. Why did the run stop, and which file stopped it?
2. Did any test file not run?

## prove_pass

1. Did the run pass? How many files and tests ran?
2. Is anything marked TODO or skipped?

## perlcritic_sev1

1. Which files have violations, and how many each?
2. List every severity 5 and severity 4 violation: file, line, policy, and the problem.
3. Which lines have more than one violation?

## cover_test, cover_report

1. Did the tests pass? If not, which assertions failed and where?
2. What is the total statement, branch and subroutine coverage?
3. Which subroutines are never called by the tests?
4. Which source lines are never executed?
5. Which branches are only partly covered?

## cpanm_installdeps

1. Which dependency did cpanm fail to install, and why?
2. How many distributions were installed?

## perldoc_function

1. What does `%v` do in a format?
2. How do you pad a number with leading zeros to width 5?
