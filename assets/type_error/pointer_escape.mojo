# An origin-bearing pointer cannot outlive the checked storage it designates.
# expect: returned pointer escapes
def escape() -> Pointer[Int]:
    var local = 7
    return Pointer(to=local)

def main():
    print(1)
