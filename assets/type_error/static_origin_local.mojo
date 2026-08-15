# Static-origin references cannot be manufactured from local storage.
# expect: cannot satisfy ImmStaticOrigin
def observe(ref[ImmStaticOrigin] value: Int):
    print(value)

def main():
    var local = 1
    observe(local)
