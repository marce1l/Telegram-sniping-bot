use teloxide::{
    dispatching::{
        dialogue::{self, GetChatId, InMemStorage}, UpdateFilterExt, UpdateHandler},
        prelude::*,
        types::{InlineKeyboardButton, InlineKeyboardMarkup},
        utils::command::{parse_command, BotCommands}
};
use lazy_static::lazy_static;
use core::fmt;
use std::{str::FromStr, sync::Mutex};

#[path ="../crypto/crypto.rs"]
mod crypto;
use crypto::alchemy_api;
use crypto::etherscan_api;

type MyDialogue = Dialogue<State, InMemStorage<State>>;
type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug)]
enum OrderType {
    Buy,
    Sell
}

impl fmt::Display for OrderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            OrderType::Buy => write!(f, "buy"),
            OrderType::Sell => write!(f, "sell")
        }
    }
}

impl FromStr for OrderType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "buy" => Ok(OrderType::Buy),
            "sell" => Ok(OrderType::Sell),
            _ => Err(())
        }
    }
}

#[derive(Clone, Debug)]
struct TradeToken {
    contract: Option<String>,
    amount: Option<f64>,
    slippage: Option<f32>,
    order_type: OrderType,
}

impl fmt::Display for TradeToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TradeToken will only be displayed if parameters are correct
        match self.order_type {
            OrderType::Buy => write!(f, "📄 Contract: {}\n💰Amount: {}\n🏷 Slippage: {}\n🟢 Order type: {}", self.contract.as_ref().unwrap(), self.amount.as_ref().unwrap(), self.slippage.as_ref().unwrap(), self.order_type),
            OrderType::Sell => write!(f, "📄 Contract: {}\n💰Amount: {}\n🏷 Slippage: {}\n🔴 Order type: {}", self.contract.as_ref().unwrap(), self.amount.as_ref().unwrap(), self.slippage.as_ref().unwrap(), self.order_type)
        }
    }
}

#[derive(Clone, Default)]
pub enum State {
    #[default]
    Start,
    Confirm,
}

#[derive(BotCommands, Clone, Debug)]
#[command(description = "These commands are supported:", rename_rule = "lowercase")]
enum Command {
    #[command(description = "help command")]
    Help,
    #[command(description = "buy ERC-20 token")]
    Buy(String),
    #[command(description = "sell ERC-20 token")]
    Sell(String),
    #[command(description = "get wallet ETH balance")]
    Balance,
    #[command(description = "get wallet ERC-20 token balances")]
    Tokens,
    #[command(description = "get current eth gas")]
    Gas,
    #[command(description = "start monitoring etherum wallets")]
    Watch(String),
    #[command(description = "cancel current command")]
    Cancel,
}

lazy_static! {
    static ref TRADE_TOKEN: Mutex<TradeToken> = Mutex::new(TradeToken { contract: None, amount: None, slippage: None, order_type: OrderType::Buy });
    static ref WATCHED_WALLETS: Mutex<Vec<String>> = Mutex::new(Vec::new());
}


#[tokio::main]
pub async fn main() {
    pretty_env_logger::init();
    log::info!("Starting command bot...");

    let bot = Bot::from_env();

    Dispatcher::builder(bot, schema())
        .dependencies(dptree::deps![InMemStorage::<State>::new()])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}


fn schema() -> UpdateHandler<Box<dyn std::error::Error + Send + Sync + 'static>> {
    use dptree::case;

    let command_handler = teloxide::filter_command::<Command, _>()
        .branch(
            case![State::Start]
            .branch(case![Command::Buy(tt)].endpoint(trade_token))
            .branch(case![Command::Sell(tt)].endpoint(trade_token))
            .branch(case![Command::Balance].endpoint(get_eth_balance))
            .branch(case![Command::Tokens].endpoint(get_erc20_balances))
            .branch(case![Command::Gas].endpoint(get_eth_gas))
        )
        .branch(case![Command::Watch(w)].endpoint(watch_wallets))
        .branch(case![Command::Help].endpoint(help))
        .branch(case![Command::Cancel].endpoint(cancel));

    let message_handler = Update::filter_message()
        .branch(command_handler)
        .branch(dptree::endpoint(invalid_state));

    let callback_query_handler = Update::filter_callback_query()
        .branch(case![State::Confirm].endpoint(confirm));

    dialogue::enter::<Update, InMemStorage<State>, State, _>()
        .branch(message_handler)
        .branch(callback_query_handler)
}

fn make_yes_no_keyboard() -> InlineKeyboardMarkup {
    let buttons: Vec<Vec<InlineKeyboardButton>> = vec![
        vec![
            InlineKeyboardButton::callback("No", "no"),
            InlineKeyboardButton::callback("Yes", "yes")]
        ];

    InlineKeyboardMarkup::new(buttons)
}

fn validate_tradetoken_args(args: &Vec<&str>, order_type: OrderType) -> Option<TradeToken> {
    let mut trade_token: TradeToken = TradeToken { contract: None, amount: None, slippage: None, order_type: order_type };

    if args.len() != 3 {
        return None;
    }

    // etherum addresses are 42 characters long (including the 0x prefix)
    if args[0].len() == 42 && args[0].starts_with("0x") {
        trade_token.contract = Some(String::from(args[0]));
    } else {
        trade_token.contract = None;
    }

    trade_token.amount = match args[1].parse() {
        Ok(v) => Some(v),
        Err(_) => None
    };

    trade_token.slippage = match args[2].parse() {
        Ok(v) => Some(v),
        Err(_) => None
    };

    let mut tt = TRADE_TOKEN.lock().unwrap();
    *tt = trade_token.clone();

    Some(trade_token)
}

fn validate_watchwallets_args(args: &Vec<&str>) -> Option<Vec<String>> {
    let mut watched_wallets: Vec<String> = vec![];

    for wallet in args {
        // etherum addresses are 42 characters long (including the 0x prefix)
        if wallet.starts_with("0x") && wallet.len() == 42 {
            watched_wallets.push(String::from(wallet.to_owned()));
        }
    }

    let mut ww = WATCHED_WALLETS.lock().unwrap();
    *ww = watched_wallets.clone();

    if watched_wallets.is_empty() { None }  else { Some(watched_wallets) }
}


async fn trade_token(bot: Bot, dialogue: MyDialogue, msg: Message) -> HandlerResult {
    let (command, args) = parse_command(msg.text().unwrap(), bot.get_me().await.unwrap().username()).unwrap();
    let trade_token: Option<TradeToken> = validate_tradetoken_args(&args, OrderType::from_str(command.to_lowercase().as_str()).unwrap());
    let mut incorrect_params: bool = false;

    match trade_token.clone() {
        Some(tt) => {
            match tt.contract {
                Some(_) => (),
                None => {
                    incorrect_params = true;
                    bot.send_message(msg.chat.id, format!("Trade cancelled: submitted contract is incorrect!")).await?;
                }
            }

            match tt.amount {
                Some(_) => (),
                None => {
                    incorrect_params = true;
                    bot.send_message(msg.chat.id, format!("Trade cancelled: submitted amount is incorrect!")).await?;
                }
            }

            match tt.slippage {
                Some(_) => (),
                None => {
                    incorrect_params = true;
                    bot.send_message(msg.chat.id, format!("Trade cancelled: submitted slippage is incorrect!")).await?;
                }
            }
        },
        None => {
            incorrect_params = true;
            bot.send_message(msg.chat.id, format!("Trade cancelled: submitted trade parameters are incorrect!")).await?;
        }
    };

    if !incorrect_params {
        bot.send_message(msg.chat.id, format!("{}", trade_token.clone().unwrap())).await?;
        bot.send_message(msg.chat.id, "Do you want to execute the transaction?").reply_markup(make_yes_no_keyboard()).await?;

        dialogue.update(State::Confirm).await?;
    } else {
        dialogue.exit().await?;
    }

    Ok(())
}

async fn confirm(bot: Bot, dialogue: MyDialogue, q: CallbackQuery) -> HandlerResult {
    let chat_id = q.chat_id().unwrap();

    match q.clone().data {
        Some(d) => {
            bot.answer_callback_query(q.id).await?;

            bot.delete_message(chat_id, q.message.unwrap().id).await?;

            if d == "yes" {
                bot.send_message(chat_id, format!("Transaction executed!")).await?;
                // TODO: handle transaction
            } else if d == "no" {
                bot.send_message(chat_id, format!("Transaction was not executed!")).await?;
            }
        }
        None => {
            bot.send_message(chat_id, format!("Something went wrong with the button handling")).await?;
        }
    }

    dialogue.exit().await?;
    Ok(())
}

async fn get_eth_balance(bot: Bot, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, format!("Your wallet balance is {}", alchemy_api::get_eth_balance().await)).await?;
    Ok(())
}

async fn watch_wallets(bot: Bot, msg: Message) -> HandlerResult {
    let (_, args) = parse_command(msg.text().unwrap(), bot.get_me().await.unwrap().username()).unwrap();
    let wallets = validate_watchwallets_args(&args);

    match wallets {
        Some(v) => {
            // TODO: handle watching wallets

            let mut message: String = String::from("Wallets to watch:\n");
            let mut counter: u8 = 0;

            for wallet in v {
                counter = counter + 1;
                message.push_str(&format!("\n{}. {}", counter, &wallet));
            }

            bot.send_message(msg.chat.id, message).await?;
        },
        None => {
            bot.send_message(msg.chat.id, format!("Watch wallets cancelled: submitted wallets are incorrect")).await?;
        }
    }

    Ok(())
}

async fn get_erc20_balances(bot: Bot, msg: Message) -> HandlerResult {
    let token_balances = alchemy_api::get_token_balances().await;
    let mut message: String = String::from("ERC-20 Token balances:\n");

    for tb in token_balances {
        println!("{}", tb);
        message.push_str(&format!("\n{}", tb));
    }

    bot.send_message(msg.chat.id, format!("{}", message)).await?;
    Ok(())
}

async fn get_eth_gas(bot: Bot, msg: Message) -> HandlerResult {
    // gas estimations based on cryptoneur.xyz/en/gas-fees-calculator
    let gwei_fee = alchemy_api::get_gas().await;
    let eth_price: f64 = etherscan_api::get_eth_price().await;

    let uniswap_v2: f64 = gwei_fee * 0.000000001 * eth_price * 152809.0 * 1.03;
    let uniswap_v3: f64 = gwei_fee * 0.000000001 * eth_price * 184523.0 * 1.03;

    let response = format!("Current eth gas is: {:.0} gwei\n\nEstimated fees:\n🦄 Uniswap V2 swap: {:.2} $\n🦄 Uniswap V3 swap: {:.2} $", gwei_fee, uniswap_v2, uniswap_v3);
    bot.send_message(msg.chat.id, response).await?;
    Ok(())
}

async fn cancel(bot: Bot, dialogue: MyDialogue, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, "Current command is cancelled").await?;
    dialogue.exit().await?;
    Ok(())
}

async fn help(bot: Bot, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, Command::descriptions().to_string()).await?;
    Ok(())
}

async fn invalid_state(bot: Bot, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, "Type /help to see availabe commands.").await?;
    Ok(())
}