# Module comment
import os
import unittest
from pathlib import Path

MAX = 100

class Greeting:
    def __init__(self, name):
        self.name = name

class _Internal:
    def __init__(self, value):
        self.value = value

def greet(name):
    """
    Return a greeting.
    """
    # Inline comment.
    return "Hello, " + name

def _helper(name):
    return name.upper()

def __helper(name):
    return name.lower()

def farewell(name):
    return "Goodbye, " + name

def test_greet():
    assert greet("World") == "Hello, World"

class TestGreeting(unittest.TestCase):
    def test_greet(self):
        self.assertEqual(greet("World"), "Hello, World")
