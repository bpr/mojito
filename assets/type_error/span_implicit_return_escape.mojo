# expect: escapes storage
# Returning a frame-local List where a Span result is expected converts
# implicitly — and rejects like the explicit construction: the view's
# refined origin is frame-local storage.
def leak() -> Span[Int, MutUnsafeAnyOrigin]:
    var xs = List[Int]()
    xs.append(10)
    return xs

def main():
    var s = leak()
    print(s[0])
