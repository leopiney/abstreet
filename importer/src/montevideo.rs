use crate::utils::{download, osmconvert};

fn input() {
    download(
        "../data/input/montevideo/osm/montevideo.osm",
        "http://download.geofabrik.de/south-america/uruguay-latest.osm.bz2",
    );
}

pub fn osm_to_raw(name: &str) {
    input();
    println!("Importing montevideo with name {}", name);
    osmconvert(
        "../data/input/montevideo/osm/montevideo.osm",
        format!("../data/input/montevideo/polygons/{}.poly", name),
        format!("../data/input/montevideo/osm/{}.osm", name),
    );

    println!("- Running convert_osm on Montevideo");
    let map = convert_osm::convert(
        convert_osm::Options {
            osm_input: format!("../data/input/montevideo/osm/{}.osm", name),
            city_name: "montevideo".to_string(),
            name: name.to_string(),

            parking_shapes: None,
            public_offstreet_parking: None,
            private_offstreet_parking: convert_osm::PrivateOffstreetParking::FixedPerBldg(1),
            sidewalks: None,
            elevation: None,
            clip: None, //Some(format!("../data/input/montevideo/polygons/{}.poly", name)),
            drive_on_right: true,
        },
        &mut abstutil::Timer::throwaway(),
    );
    let output = format!("../data/input/raw_maps/{}.bin", name);
    println!("- Saving Montevideo's output to {}", output);
    abstutil::write_binary(output, &map);
}
