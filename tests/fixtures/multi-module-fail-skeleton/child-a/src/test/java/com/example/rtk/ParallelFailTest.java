package com.example.rtk;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class ParallelFailTest {
    @Test
    void reactorDiagnostic() {
        assertEquals(1, 1 + 1, "parallel reactor diagnostic");
    }
}
