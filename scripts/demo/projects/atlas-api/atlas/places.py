"""Places: an in-memory catalogue, searched by name and distance."""

from dataclasses import dataclass
from math import asin, cos, radians, sin, sqrt


@dataclass(frozen=True)
class Place:
    id: str
    name: str
    lat: float
    lon: float
    kind: str


CATALOGUE = [
    Place("p1", "Blue Bottle Coffee", 37.7764, -122.4232, "cafe"),
    Place("p2", "Dolores Park", 37.7596, -122.4269, "park"),
    Place("p3", "City Lights Books", 37.7976, -122.4065, "bookshop"),
    Place("p4", "Ferry Building", 37.7955, -122.3937, "market"),
    Place("p5", "Tartine Bakery", 37.7614, -122.4241, "bakery"),
]


def distance_km(a: Place, lat: float, lon: float) -> float:
    """Great-circle distance (haversine)."""
    dlat, dlon = radians(lat - a.lat), radians(lon - a.lon)
    h = sin(dlat / 2) ** 2 + cos(radians(a.lat)) * cos(radians(lat)) * sin(dlon / 2) ** 2
    return 2 * 6371 * asin(sqrt(h))


def search(query: str = "", near: tuple[float, float] | None = None, limit: int = 10) -> list[Place]:
    found = [p for p in CATALOGUE if query.lower() in p.name.lower()]
    if near:
        found.sort(key=lambda p: distance_km(p, *near))
    return found[:limit]
