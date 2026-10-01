import unittest

from arithmetic import double


class ArithmeticTests(unittest.TestCase):
    def test_double(self):
        self.assertEqual(double(21), 42)
        self.assertEqual(double(-3), -6)
        self.assertEqual(double(0), 0)


if __name__ == "__main__":
    unittest.main()
