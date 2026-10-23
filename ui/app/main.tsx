// @title Pocket New York
// One screen with a pad: the PS Vita (and the PSP's shape). pocket.json compiles this entry wherever no other
// presentation matches the device's modality.
import { mount } from "@pocketjs/framework";
import SingleScreen from "./presentations/single.tsx";

mount(() => <SingleScreen />);
