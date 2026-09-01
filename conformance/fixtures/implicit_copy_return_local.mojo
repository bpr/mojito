# Pinned head (2026-08-30): returning a non-ImplicitlyCopyable local
# requires `result^` (or `.copy()`).
def make() -> List[Int]:
    var result = List[Int]()
    result.append(1)
    return result

def main():
    print(len(make()))
