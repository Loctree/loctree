from app.service import normalize


def test_normalize() -> None:
    assert normalize(" Vet ") == "vet"
