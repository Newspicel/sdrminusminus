Release builds replace bin/, lib/ and licenses/ with the pinned private SoapySDR runtime.

Each of those three is mapped into the bundle on its own, and no two of the mappings may
overlap: the Windows bundlers key their resource table by source path, so a file reachable
through two mappings is installed to exactly one of the two destinations, picked by the hash
order of the resource map. That is how SoapySDR.dll stopped being installed beside the
executable, which the loader needs it to be.
