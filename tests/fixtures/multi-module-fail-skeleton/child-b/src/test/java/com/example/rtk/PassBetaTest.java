package com.example.rtk;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class PassBetaTest {
    @Test
    void multiplies() {
        assertEquals(6, 2 * 3);
    }

    @Test
    void negates() {
        assertEquals(-5, -(5));
    }
}
