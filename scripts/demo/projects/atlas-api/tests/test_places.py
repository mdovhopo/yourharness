import unittest

from atlas.places import search


class SearchTest(unittest.TestCase):
    def test_by_name(self):
        self.assertEqual([p.id for p in search("park")], ["p2"])

    def test_nearest_first(self):
        ids = [p.id for p in search(near=(37.7600, -122.4250))]
        self.assertEqual(ids[0], "p2")


if __name__ == "__main__":
    unittest.main()
