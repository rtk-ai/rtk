package com.example.rtk;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class CalcTest {
    private final Calc calc = new Calc();

    @Test
    void failOne() {
        assertEquals(5, calc.add(2, 2), "failOne: addition should equal five");
    }

    @Test
    void failTwo() {
        throw new IllegalStateException("failTwo: induced error for fixture capture");
    }

    @Test
    void passes() {
        assertEquals(4, calc.add(2, 2));
    }
}
