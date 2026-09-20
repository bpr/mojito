# expect: increment required
# A violated arithmetic `where` rejects the application with the clause's own
# message.
def check[n: Int, m: Int]() -> Int where (n + 1 == m, "increment required"):
    return m


def main():
    print(check[3, 5]())
