# expect: use of invalidated interior reference 'view' to 's["bytes"]'
# A view of a whole local's owned interior (`s.strip()` is a `StringSlice`
# over `origin_of(s)._get_owned_interior["bytes"]`) lends `s["bytes"]`: a
# store to `s` stales it, and the view's next use is rejected, as at the pin.
def main():
    var s = String("  ab  ")
    var view = s.strip()
    s = String("q")
    print(view)
