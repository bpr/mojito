# Strict-subset gap: upstream accepts mutating a Dict while a stored
# `keys()` view is live (its pointer-backed view then observes the
# concurrent insertion — printing 1, 2, 9 — and can corrupt on a rehash),
# while Mojito's borrowing view holds a loan on the dictionary and rejects
# the mutation outright ("access to 'd' conflicts with live reference")
# to keep view iteration coherent.
def main() raises:
    var d: Dict[Int, Int] = {1: 10, 2: 20}
    var kv = d.keys()
    d[9] = 90
    for k in kv:
        print(k)
