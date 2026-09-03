package com.example.rtk;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class PassFourTest {
    private final Calc calc = new Calc();

    @Test
    void addsCommutatively() {
        assertEquals(calc.add(3, 4), calc.add(4, 3));
    }

    @Test
    void addsZero() {
        assertEquals(7, calc.add(7, 0));
    }
}
