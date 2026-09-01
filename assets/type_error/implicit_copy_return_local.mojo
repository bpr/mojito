# expect: cannot be implicitly copied
# Returning a local of a Copyable-only type implicitly copies it; upstream
# performs no last-use move, so the return must spell `result^`.
def make() -> List[Int]:
    var result = List[Int]()
    result.append(1)
    return result

def main():
    print(len(make()))
