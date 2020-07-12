use log::info;
use solana_sdk::banks_client::{ws::start_client, BanksClient};
use solana_sdk::{
    banks_client::{context, BanksClientExt},
    pubkey::Pubkey,
};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use yew::prelude::*;
use yewtil::future::LinkFuture;

struct Model {
    client: Option<Rc<RefCell<BanksClient>>>,
    link: ComponentLink<Self>,
    balance: u64,
}

enum Msg {
    GetBalance,
    Balance(u64),
    Connected(BanksClient),
}

impl Component for Model {
    type Message = Msg;
    type Properties = ();
    fn create(_: Self::Properties, link: ComponentLink<Self>) -> Self {
        link.send_future(async {
            info!("Start client");
            let (client, dispatch) = start_client("ws://127.0.0.1:8901").await.expect("failed");
            wasm_bindgen_futures::spawn_local(dispatch);
            info!("Client connected");
            Msg::Connected(client)
        });

        Self {
            link,
            client: None,
            balance: 0,
        }
    }

    fn update(&mut self, msg: Self::Message) -> ShouldRender {
        match msg {
            Msg::Connected(client) => self.client = Some(Rc::new(RefCell::new(client))),
            Msg::Balance(balance) => self.balance = balance,
            Msg::GetBalance => {
                if let Some(client) = &self.client {
                    let link = self.link.clone();
                    let client = client.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let mut client = client.borrow_mut();
                        let window =
                            web_sys::window().expect("should have a window in this context");
                        let performance = window
                            .performance()
                            .expect("performance should be available");
                        let millis = performance.now() as u64;
                        let context = context::with_deadline(
                            std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis),
                        );
                        let balance = client.get_balance(context, Pubkey::default());
                        let balance = balance.await.expect("no balance");
                        link.send_message(Msg::Balance(balance));
                    });
                }
                return false;
            }
        }
        true
    }

    fn change(&mut self, _props: Self::Properties) -> ShouldRender {
        // Should only return "true" if new properties are different to
        // previously received properties.
        // This component has no properties so we will always return "false".
        false
    }

    fn view(&self) -> Html {
        html! {
            <div>
                <button onclick=self.link.callback(|_| Msg::GetBalance)>{ "Get Balance" }</button>
                <p>{ self.balance }</p>
            </div>
        }
    }
}

#[wasm_bindgen(start)]
pub fn run_app() {
    yew::initialize();
    wasm_logger::init(wasm_logger::Config::default());
    App::<Model>::new().mount_to_body();
}
